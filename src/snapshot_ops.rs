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

use std::process::Command as ExecProcess;
use which::which;

pub fn take_snapshot(
    config: &Config,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    let selected_datasets = vec![NativeDatasetType::MostProximate];

    let mounts_for_files: Vec<PathData> =
        get_mounts_for_files(config, &config.paths, &selected_datasets)?;

    fn exec_snapshot(
        config: &Config,
        zfs_command: &PathBuf,
        mounts_for_files: &[PathData],
    ) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
        let exec_command = zfs_command;
        let now = SystemTime::now();

        let res: Result<(), HttmError> = mounts_for_files.iter().try_for_each(|mount| {
            let opt_dataset = match &config.snap_point {
                SnapPoint::Native(native_datasets) => {
                    match native_datasets.map_of_datasets.get(&mount.path_buf) {
                        Some((dataset, fs_type)) => {
                            if let FilesystemType::Zfs = fs_type {
                                Some(dataset)
                            } else {
                                None
                            }
                        }
                        None => None,
                    }
                }
                SnapPoint::UserDefined(_) => None,
            };

            let dataset = match opt_dataset {
                Some(dataset) => dataset,
                None => unreachable!("Unable to parse dataset!"),
            };

            let snapshot_name = format!(
                "{}@snap_{}_httmSnapFileMount",
                dataset,
                timestamp_file(&now)
            );

            let args = vec!["snapshot", &snapshot_name];

            let output = ExecProcess::new(exec_command)
                .args(&args)
                .output()
                .unwrap()
                .stderr;

            let err = std::str::from_utf8(
                // seems to exec Ok unless command DNE
                &output,
            )
            .unwrap();

            if !err.is_empty() {
                return Err(HttmError::new(&format!(
                    "httm was unable to take a snapshot.  \
                    See the following context: {}",
                    err
                )));
            } else {
                println!("httm took a snapshot at: {}", &snapshot_name)
            }

            Ok(())
        });

        match res {
            Ok(_) => {
                std::process::exit(0);
            }
            Err(err) => Err(Box::new(err)),
        }
    }

    if let Ok(zfs_command) = which("zfs") {
        exec_snapshot(config, &zfs_command, &mounts_for_files)
    } else {
        Err(
            HttmError::new("zfs command not found. Make sure the command 'zfs' is in your path.")
                .into(),
        )
    }
}
