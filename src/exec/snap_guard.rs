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

use std::path::Path;
use std::process::Command as ExecProcess;
use std::time::SystemTime;

use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{get_date, DateFormat};
use crate::print_output_buf;
use crate::GLOBAL_CONFIG;

pub enum PrecautionarySnapType {
    Pre,
    Post,
}

pub struct SnapGuard;

impl SnapGuard {
    pub fn exec_rollback(pre_exec_snap_name: &str, zfs_command: &Path) -> HttmResult<()> {
        let mut process_args = vec!["rollback", "-r"];
        process_args.push(pre_exec_snap_name);

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

    pub fn exec_snap(
        zfs_command: &Path,
        dataset_name: &str,
        snap_name: &str,
        snap_type: PrecautionarySnapType,
    ) -> HttmResult<String> {
        let mut process_args = vec!["snapshot".to_owned()];

        let timestamp = get_date(
            GLOBAL_CONFIG.requested_utc_offset,
            &SystemTime::now(),
            DateFormat::Timestamp,
        );

        let new_snap_name = match &snap_type {
            PrecautionarySnapType::Pre => {
                // all snapshots should have the same timestamp
                let new_snap_name = format!(
                    "{}@snap_pre_{}_httmSnapRollForward",
                    dataset_name, timestamp
                );

                new_snap_name
            }
            PrecautionarySnapType::Post => {
                let new_snap_name = format!(
                    "{}@snap_post_{}_:{}:_httmSnapRollForward",
                    dataset_name, timestamp, snap_name
                );

                new_snap_name
            }
        };

        process_args.push(new_snap_name.clone());

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
                PrecautionarySnapType::Pre => {
                    format!(
                        "httm took a pre-execution snapshot named: {}\n",
                        &new_snap_name
                    )
                }
                PrecautionarySnapType::Post => {
                    format!(
                        "httm took a post-execution snapshot named: {}\n",
                        &new_snap_name
                    )
                }
            };

            print_output_buf(output_buf)?;

            Ok(new_snap_name)
        }
    }
}
