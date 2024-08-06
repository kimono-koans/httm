//       ___           ___           ___           ___
//      /\__\         /\  \         /\  \         /\__\
//     /:/  /         \:\  \        \:\  \       /::|  |
//    /:/__/           \:\  \        \:\  \     /:|:|  |
//   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
//  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
//  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
//       \::/  /    /:/  /        /:/  /            /:/  /
//       /:/  /     \/__/         \/__/            /:/  /
//      /:/  /                                    /:/  /
//      \/__/                                     \/__/
//
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::config::generate::PrintMode;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{date_string, delimiter, print_output_buf, DateFormat};
use crate::lookup::file_mounts::MountDisplay;
use crate::lookup::file_mounts::MountsForFiles;
use crate::parse::mounts::FilesystemType;
use crate::GLOBAL_CONFIG;
use std::collections::BTreeMap;
use std::time::SystemTime;

use super::run_command::RunZFSCommand;

pub struct SnapshotMounts;

impl SnapshotMounts {
    pub fn exec(requested_snapshot_suffix: &str) -> HttmResult<()> {
        let mounts_for_files: MountsForFiles = MountsForFiles::new(&MountDisplay::Target)?;

        let map_snapshot_names =
            Self::snapshot_names(&mounts_for_files, requested_snapshot_suffix)?;

        map_snapshot_names.values().try_for_each(|snapshot_names| {
            let run_zfs = RunZFSCommand::new()?;
            run_zfs.snapshot(snapshot_names)?;

            let output_buf: String = snapshot_names
                .iter()
                .map(|snap_name| {
                    if matches!(
                        GLOBAL_CONFIG.print_mode,
                        PrintMode::RawNewline | PrintMode::RawZero
                    ) {
                        let delimiter = delimiter();
                        format!("{}{delimiter}", &snap_name)
                    } else {
                        format!("httm took a snapshot named: {}\n", &snap_name)
                    }
                })
                .collect();

            print_output_buf(&output_buf)
        })?;

        Ok(())
    }

    pub fn pool_from_snap_name(snapshot_name: &str) -> HttmResult<String> {
        // split on "/" why?  because a snap looks like: rpool/kimono@snap...
        // splits according to pool name, then the rest of the snap name
        match snapshot_name.split_once('/') {
            Some((pool_name, _snap_name)) => Ok(pool_name.into()),
            None => {
                let msg = format!(
                    "Could not determine pool name from the constructed snapshot name: {snapshot_name}"
                );
                Err(HttmError::new(&msg).into())
            }
        }
    }

    fn snapshot_names(
        mounts_for_files: &MountsForFiles,
        requested_snapshot_suffix: &str,
    ) -> HttmResult<BTreeMap<String, Vec<String>>> {
        // all snapshots should have the same timestamp
        let timestamp = date_string(
            GLOBAL_CONFIG.requested_utc_offset,
            &SystemTime::now(),
            DateFormat::Timestamp,
        );

        let vec_snapshot_names: Vec<String> = mounts_for_files
            .iter()
            .flat_map(|prox| prox.datasets_of_interest())
            .map(|mount| {
                let dataset = match &GLOBAL_CONFIG.dataset_collection.opt_map_of_aliases {
                    None => {
                        match GLOBAL_CONFIG
                            .dataset_collection
                            .map_of_datasets
                            .get(mount)
                        {
                            Some(dataset_info) => {
                                if let FilesystemType::Zfs = dataset_info.fs_type {
                                    Ok(dataset_info.source.to_string_lossy())
                                } else {
                                    Err(HttmError::new(
                                        "httm does not currently support snapshot-ing non-ZFS filesystems.",
                                    ))
                                }
                            }
                            None => {
                                return Err(HttmError::new(
                                    "httm was unable to parse dataset from mount!",
                                ))
                            }
                        }
                    }
                    Some(_) => return Err(HttmError::new(
                        "httm does not currently support snapshot-ing user defined mount points.",
                    )),
                }?;

                let snapshot_name = format!(
                    "{}@snap_{}_{}",
                    dataset, timestamp, requested_snapshot_suffix,
                );

                Ok(snapshot_name)
            })
            .collect::<Result<Vec<String>, HttmError>>()?;

        if vec_snapshot_names.is_empty() {
            return Err(HttmError::new(
                "httm could not generate any valid snapshot names from requested input.  Quitting.",
            )
            .into());
        }

        // why all this garbage with BTreeMaps, etc.? ZFS will not allow one to take snapshots
        // with the same name, at the same time, across pools.  Since we don't really care, we break
        // the snapshots into groups by pool name and then just take snapshots for each pool
        let map_snapshot_names: BTreeMap<String, Vec<String>> = vec_snapshot_names
            .into_iter()
            .into_group_map_by(|snapshot_name| {
                Self::pool_from_snap_name(snapshot_name).unwrap_or_else(|err| {
                    eprintln!("ERROR: {:?}", err);
                    std::process::exit(1)
                })
            })
            .iter_mut()
            .map(|(key, group)| {
                group.sort();
                group.dedup();
                (key.clone(), group.clone())
            })
            .collect();

        if map_snapshot_names.is_empty() {
            return Err(HttmError::new("httm could not generate a valid map of snapshot names from the requested input.  Quitting.").into());
        }

        Ok(map_snapshot_names)
    }
}
