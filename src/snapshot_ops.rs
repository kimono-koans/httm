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

use std::{path::PathBuf, time::SystemTime};

use crate::utility::timestamp_file;
use crate::versions_lookup::{get_mounts_for_files, NativeDatasetType};
use crate::{Config, HttmError, PathData};
use crate::{FilesystemType, SnapPoint};

use itertools::Itertools;
use std::process::Command as ExecProcess;
use which::which;

pub fn take_snapshot(
    config: &Config,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    fn exec_zfs_snapshot(
        config: &Config,
        zfs_command: &PathBuf,
        mounts_for_files: &[PathData],
    ) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
        // all snapshots should have the same timestamp
        let now = SystemTime::now();

        let vec_snaps: Result<Vec<String>, HttmError> = mounts_for_files.iter().map(|mount| {
            let dataset: String = match &config.snap_point {
                SnapPoint::Native(native_datasets) => {
                    match native_datasets.map_of_datasets.get(&mount.path_buf) {
                        Some((dataset, fs_type)) => {
                            if let FilesystemType::Zfs = fs_type {
                                Ok(dataset.to_owned())
                            } else {
                                return Err(HttmError::new("httm does not currently support snapshot-ing non-ZFS filesystems."))
                            }
                        }
                        None => return Err(HttmError::new("Unable to parse dataset from mount!")),
                    }
                }
                SnapPoint::UserDefined(_) => return Err(HttmError::new("httm does not currently support snapshot-ing user defined mount points.")),
            }?;

            let snapshot_name = format!(
                "{}@snap_{}_httmSnapFileMount",
                dataset,
                timestamp_file(&now)
            );

            Ok(snapshot_name)
        }).collect::<Result<Vec<String>, HttmError>>();

        let snap_names: Vec<String> = vec_snaps?.into_iter().dedup().collect();

        let mut args = vec!["snapshot".to_owned()];
        args.extend(snap_names.clone());

        let process_output = ExecProcess::new(zfs_command).args(&args).output().unwrap();

        // fn seems to exec Ok unless command DNE, so unwrap is okay here
        let err = std::str::from_utf8(&process_output.stderr).unwrap().trim();

        if !err.is_empty() {
            return Err(HttmError::new(&format!(
                "httm was unable to take a snapshot/s. \
                The 'zfs' command issued the following error: {}",
                err
            ))
            .into());
        } else {
            snap_names.iter().for_each(|snap_name| {
                println!("httm took a snapshot named: {}", &snap_name);
            });
            std::process::exit(0);
        }
    }

    let selected_datasets = vec![NativeDatasetType::MostProximate];

    let mounts_for_files: Vec<PathData> =
        get_mounts_for_files(config, &config.paths, &selected_datasets)?;

    if let Ok(zfs_command) = which("zfs") {
        exec_zfs_snapshot(config, &zfs_command, &mounts_for_files)
    } else {
        Err(
            HttmError::new("zfs command not found. Make sure the command 'zfs' is in your path.")
                .into(),
        )
    }
}
