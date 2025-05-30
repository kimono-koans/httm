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

use crate::GLOBAL_CONFIG;
use crate::config::generate::PrintMode;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{DateFormat, date_string, delimiter, print_output_buf};
use crate::lookup::file_mounts::{MountDisplay, MountsForFiles};
use crate::zfs::run_command::{RunZFSCommand, ZfsAllowPriv};
use std::collections::BTreeMap;
use std::time::SystemTime;

pub struct SnapshotMounts;

impl SnapshotMounts {
    pub fn exec(requested_snapshot_suffix: &str) -> HttmResult<()> {
        let mounts_for_files: MountsForFiles = MountsForFiles::new(&MountDisplay::Target)?;

        let map_snapshot_names =
            Self::snapshot_names(&mounts_for_files, requested_snapshot_suffix)?;

        let run_zfs = RunZFSCommand::new()?;

        map_snapshot_names.values().try_for_each(|snapshot_names| {
            run_zfs.snapshot(snapshot_names)?;

            let output_buf: String = snapshot_names
                .iter()
                .map(|snap_name| {
                    if let PrintMode::Raw(_) = GLOBAL_CONFIG.print_mode {
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
        match snapshot_name.split_once('@') {
            Some((dataset_name, _snap_name)) => {
                // split on "/" why?  because a snap looks like: rpool/kimono@snap...
                // splits according to pool name, then the rest of the snap name
                match dataset_name.split_once('/') {
                    Some((pool_name, _the_rest)) => Ok(pool_name.into()),
                    // what if no "/", then pool name is dataset name
                    None => Ok(dataset_name.into()),
                }
            }
            None => {
                let description = format!(
                    "Could not determine pool name from the constructed snapshot name: {snapshot_name}"
                );
                HttmError::from(description).into()
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
            .map(|prox| {
                let path_data = prox.path_data();

                let fs_name = ZfsAllowPriv::Snapshot
                    .from_opt_proximate_dataset(&path_data, Some(prox.proximate_dataset()))
                    .map_err(|err| HttmError::from(err))?;

                let snapshot_name = format!(
                    "{}@snap_{}_{}",
                    fs_name.to_string_lossy(),
                    timestamp,
                    requested_snapshot_suffix,
                );

                Ok(snapshot_name)
            })
            .collect::<Result<Vec<String>, HttmError>>()?;

        if vec_snapshot_names.is_empty() {
            return HttmError::new(
                "httm could not generate any valid snapshot names from requested input.  Quitting.",
            )
            .into();
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
            return HttmError::new("httm could not generate a valid map of snapshot names from the requested input.  Quitting.").into();
        }

        Ok(map_snapshot_names)
    }
}
