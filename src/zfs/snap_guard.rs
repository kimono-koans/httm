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

use crate::library::results::HttmResult;
use crate::library::utility::{date_string, DateFormat};
use crate::zfs::run_command::ZfsAllowPriv;
use crate::{print_output_buf, GLOBAL_CONFIG};
use std::path::Path;
use std::time::SystemTime;

use super::run_command::RunZFSCommand;

pub enum PrecautionarySnapType {
    PreRollForward,
    PostRollForward(String),
    PreRestore,
}

impl TryFrom<&Path> for SnapGuard {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(path: &Path) -> HttmResult<Self> {
        // guards the ZFS action, returns source dataset
        let source = ZfsAllowPriv::Snapshot.from_path(&path)?;

        SnapGuard::new(&source.to_string_lossy(), PrecautionarySnapType::PreRestore)
    }
}

pub struct SnapGuard {
    new_snap_name: String,
    dataset_name: String,
}

impl SnapGuard {
    pub fn new(dataset_name: &str, snap_type: PrecautionarySnapType) -> HttmResult<Self> {
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

        let run_zfs = RunZFSCommand::new()?;

        run_zfs.snapshot(&[new_snap_name.clone()])?;

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

    pub fn rollback(&self) -> HttmResult<()> {
        ZfsAllowPriv::Rollback.from_fs_name(&self.dataset_name)?;

        let run_zfs = RunZFSCommand::new()?;
        run_zfs.rollback(&[self.new_snap_name.to_owned()])?;

        Ok(())
    }
}
