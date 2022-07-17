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
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::collections::BTreeMap;
use std::{path::Path, time::SystemTime};

use itertools::Itertools;
use std::process::Command as ExecProcess;
use which::which;

use crate::utility::{print_output_buf, timestamp_file, HttmError, PathData};
use crate::versions_lookup::{get_mounts_for_files, SnapshotDatasetType};
use crate::{AHashMap as HashMap, Config};

use crate::{DatasetCollection, FilesystemType};

pub fn take_snapshot(
    config: &Config,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    fn exec_zfs_snapshot(
        config: &Config,
        zfs_command: &Path,
        mounts_for_files: &BTreeMap<PathData, Vec<PathData>>,
    ) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
        // all snapshots should have the same timestamp
        let timestamp = timestamp_file(&SystemTime::now());

        let vec_snapshot_names: Vec<String> = mounts_for_files
            .iter()
            .flat_map(|(_pathdata, datasets)| datasets)
            .map(|mount| {
            let dataset: String = match &config.dataset_collection {
                DatasetCollection::AutoDetect(detected_datasets) => {
                    match detected_datasets.map_of_datasets.get(&mount.path_buf) {
                        Some((dataset, fs_type)) => {
                            if let FilesystemType::Zfs = fs_type {
                                Ok(dataset.to_owned())
                            } else {
                                return Err(HttmError::new("httm does not currently support snapshot-ing non-ZFS filesystems."))
                            }
                        }
                        None => return Err(HttmError::new("httm was unable to parse dataset from mount!")),
                    }
                }
                DatasetCollection::UserDefined(_) => return Err(HttmError::new("httm does not currently support snapshot-ing user defined mount points.")),
            }?;

            let snapshot_name = format!(
                "{}@snap_{}_httmSnapFileMount",
                dataset,
                timestamp,
            );

            Ok(snapshot_name)
        }).collect::<Result<Vec<String>, HttmError>>()?;

        // why all this garbage with hashmaps, etc.? ZFS will not allow one to take snapshots
        // with the same name, at the same time, across pools.  Since we don't really care, we break
        // the snapshots into groups by pool name and then just take snapshots for each pool
        let map_snapshot_names: HashMap<String, Vec<String>> = vec_snapshot_names
            .into_iter()
            .into_group_map_by(|snapshot_name| {
                let (pool_name, _rest) = snapshot_name
                    .split_once('/')
                    .unwrap_or((snapshot_name.as_ref(), snapshot_name.as_ref()));
                pool_name.to_owned()
            })
            .iter_mut()
            .map(|(key, group)| {
                group.sort();
                group.dedup();
                (key.to_owned(), group.to_owned())
            })
            .collect();

        map_snapshot_names.iter().try_for_each( |(_pool_name, snapshot_names)| {
            let mut process_args = vec!["snapshot".to_owned()];
            process_args.extend(snapshot_names.clone());

            let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
            let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

            // stderr_string is a string not an error, so here we build an err or output
            if !stderr_string.is_empty() {
                let msg = if stderr_string.contains("cannot create snapshots : permission denied") {
                    "httm must have root privileges to snapshot a filesystem".to_owned()
                } else {
                    "httm was unable to take snapshots. The 'zfs' command issued the following error: ".to_owned() + stderr_string
                };

                Err(HttmError::new(&msg).into())
            } else {
                let output_buf = snapshot_names
                    .iter()
                    .map(|snap_name| format!("httm took a snapshot named: {}\n", &snap_name))
                    .collect();
                print_output_buf(output_buf)
            }
        })?;

        std::process::exit(0)
    }

    // don't want to request alt replicated mounts, though we may in opt_mount_for_file mode
    let selected_datasets = vec![SnapshotDatasetType::MostProximate];

    let mounts_for_files: BTreeMap<PathData, Vec<PathData>> =
        get_mounts_for_files(config, &config.paths, &selected_datasets)?;

    if let Ok(zfs_command) = which("zfs") {
        exec_zfs_snapshot(config, &zfs_command, &mounts_for_files)
    } else {
        Err(
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
                .into(),
        )
    }
}
