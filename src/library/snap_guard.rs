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
use crate::data::paths::PathDeconstruction;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::get_zfs_command;
use crate::library::utility::user_has_effective_root;
use crate::library::utility::{date_string, DateFormat};
use crate::{print_output_buf, GLOBAL_CONFIG};
use std::path::Path;
use std::process::Command as ExecProcess;
use std::time::SystemTime;

pub enum PrecautionarySnapType {
    PreRollForward,
    PostRollForward(String),
    PreRestore,
}

impl TryFrom<&Path> for SnapGuard {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(path: &Path) -> HttmResult<Self> {
        ZfsAllowPriv::Snapshot.from_path(&path)?;

        let pathdata = PathData::from(path);

        let dataset_name = match pathdata.source(None) {
            Some(source) => source,
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
    new_snap_name: String,
    dataset_name: String,
}

impl SnapGuard {
    pub fn new(dataset_name: &str, snap_type: PrecautionarySnapType) -> HttmResult<Self> {
        let zfs_command = get_zfs_command()?;

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

            print_output_buf(&output_buf)?;

            Ok(SnapGuard {
                new_snap_name,
                dataset_name: dataset_name.to_string(),
            })
        }
    }

    pub fn rollback(&self) -> HttmResult<()> {
        ZfsAllowPriv::Rollback.from_fs_name(&self.dataset_name)?;

        let zfs_command = get_zfs_command()?;
        let process_args = vec!["rollback", "-r", &self.new_snap_name];

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

pub enum ZfsAllowPriv {
    Snapshot,
    Rollback,
}

impl ZfsAllowPriv {
    pub fn from_path(&self, new_file_path: &Path) -> HttmResult<()> {
        let pathdata = PathData::from(new_file_path);

        let Some(fs_name) = pathdata.source(None) else {
            let msg = format!(
                "Could not determine dataset name from path given: {:?}",
                new_file_path
            );
            return Err(HttmError::new(&msg).into());
        };

        Self::from_fs_name(&self, &fs_name.to_string_lossy())
    }

    pub fn from_fs_name(&self, fs_name: &str) -> HttmResult<()> {
        let msg = match self {
            ZfsAllowPriv::Rollback => "A rollback after a restore action",
            ZfsAllowPriv::Snapshot => "A snapshot guard before restore action",
        };

        if let Err(root_error) = user_has_effective_root(msg) {
            if let Err(_allow_priv_error) = self.user_has_zfs_allow_priv(fs_name) {
                return Err(root_error);
            }
        }

        Ok(())
    }

    fn as_zfs_cmd_strings(&self) -> &[&str] {
        match self {
            ZfsAllowPriv::Rollback => &["rollback"],
            ZfsAllowPriv::Snapshot => &["snapshot", "mount"],
        }
    }

    fn user_has_zfs_allow_priv(&self, fs_name: &str) -> HttmResult<()> {
        let zfs_command = get_zfs_command()?;

        let process_args = vec!["allow", fs_name];

        let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();
        let stdout_string = std::str::from_utf8(&process_output.stdout)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = "httm was unable to determine 'zfs allow' for the path given. The 'zfs' command issued the following error: ".to_owned() + stderr_string;

            return Err(HttmError::new(&msg).into());
        }

        let user_name = std::env::var("USER")?;

        if !stdout_string.contains(&user_name)
            || !self
                .as_zfs_cmd_strings()
                .iter()
                .all(|p| stdout_string.contains(p))
        {
            let msg = "User does not have 'zfs allow' privileges for the path given.";

            return Err(HttmError::new(msg).into());
        }

        Ok(())
    }
}
