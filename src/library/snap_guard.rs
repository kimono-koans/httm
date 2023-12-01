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

use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{date_string, DateFormat};
use crate::{print_output_buf, GLOBAL_CONFIG};
use std::path::Path;
use std::process::Command as ExecProcess;
use std::time::SystemTime;
use which::which;

pub enum PrecautionarySnapType {
    PreRollForward,
    PostRollForward(String),
    PreRestore,
}

impl TryFrom<&Path> for SnapGuard {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(path: &Path) -> HttmResult<Self> {
        let pathdata = PathData::from(path);
        let dataset_mount =
            pathdata.proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)?;

        let dataset_name = match GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(dataset_mount)
        {
            Some(md) => &md.source,
            None => {
                return Err(HttmError::new("Could not obtain source dataset for mount: ").into())
            }
        };

        SnapGuard::new(
            &dataset_name.to_string_lossy(),
            PrecautionarySnapType::PreRestore,
        )
    }
}

pub struct SnapGuard {
    inner: String,
}

impl SnapGuard {
    pub fn new(dataset_name: &str, snap_type: PrecautionarySnapType) -> HttmResult<Self> {
        let zfs_command = which("zfs")?;

        let timestamp = date_string(
            GLOBAL_CONFIG.requested_utc_offset,
            &SystemTime::now(),
            DateFormat::Timestamp,
        );

        let new_snap_name = match &snap_type {
            PrecautionarySnapType::PreRollForward => {
                // all snapshots should have the same timestamp
                let new_snap_name = format!(
                    "{}@snap_pre_{}_httmSnapRollForward",
                    dataset_name, timestamp
                );

                new_snap_name
            }
            PrecautionarySnapType::PostRollForward(additional_snap_info_str) => {
                let new_snap_name = format!(
                    "{}@snap_post_{}_:{}:_httmSnapRollForward",
                    dataset_name, timestamp, additional_snap_info_str
                );

                new_snap_name
            }
            PrecautionarySnapType::PreRestore => {
                // all snapshots should have the same timestamp
                let new_snap_name =
                    format!("{}@snap_pre_{}_httmSnapRestore", dataset_name, timestamp);

                new_snap_name
            }
        };

        let process_args = vec!["snapshot".to_owned(), new_snap_name.clone()];

        let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot create snapshots : permission denied") {
                "httm must have root privileges to snapshot a filesystem".to_owned()
            } else {
                "httm was unable to take snapshots. The 'zfs' command issued the following error: "
                    .to_owned()
                    + stderr_string
            };

            Err(HttmError::new(&msg).into())
        } else {
            let output_buf = match &snap_type {
                PrecautionarySnapType::PreRollForward | PrecautionarySnapType::PreRestore => {
                    format!(
                        "httm took a pre-execution snapshot named: {}\n",
                        &new_snap_name
                    )
                }
                PrecautionarySnapType::PostRollForward(_) => {
                    format!(
                        "httm took a post-execution snapshot named: {}\n",
                        &new_snap_name
                    )
                }
            };

            print_output_buf(output_buf)?;

            Ok(SnapGuard {
                inner: new_snap_name,
            })
        }
    }

    pub fn rollback(&self) -> HttmResult<()> {
        let zfs_command = which("zfs")?;
        let process_args = vec!["rollback", "-r", &self.inner];

        let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                "httm may need root privileges to 'zfs rollback' a filesystem".to_owned()
            } else {
                "httm was unable to rollback the snapshot name. The 'zfs' command issued the following error: ".to_owned() + stderr_string
            };

            return Err(HttmError::new(&msg).into());
        }

        Ok(())
    }
}
