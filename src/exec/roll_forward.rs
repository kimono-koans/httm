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

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::time::SystemTime;

use rayon::prelude::*;
use which::which;

use crate::config::generate::ListSnapsOfType;
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::copy_recursive;
use crate::library::utility::{get_date, DateFormat};
use crate::lookup::versions::{SnapDatasetType, VersionsMap};
use crate::print_output_buf;
use crate::GLOBAL_CONFIG;

pub enum PrecautionarySnapType {
    Pre,
    Post(String),
}

pub struct RollForward;

impl RollForward {
    pub fn exec(full_snap_name: &str) -> HttmResult<()> {
        let zfs_command = if let Ok(zfs_command) = which("zfs") {
            zfs_command
        } else {
            return Err(HttmError::new(
                "'zfs' command not found. Make sure the command 'zfs' is in your path.",
            )
            .into());
        };

        let (dataset_name, snap_name) = full_snap_name
            .split_once('@')
            .expect("Input is not a valid ZFS dataset name.");

        let zfs_diff_str = Self::exec_diff(full_snap_name, &zfs_command)?;

        let diff_map = DiffMap::new(&zfs_diff_str, dataset_name, snap_name)?;

        RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Pre,
        )?;

        diff_map.roll_forward()?;

        RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Post(full_snap_name.to_owned()),
        )
    }

    fn exec_diff(full_snapshot_name: &str, zfs_command: &Path) -> HttmResult<String> {
        let mut process_args = vec!["diff", "-H"];
        process_args.push(full_snapshot_name);

        let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                "httm may need root privileges to 'zfs diff' a filesystem".to_owned()
            } else {
                "httm was unable to diff the snapshot name. The 'zfs' command issued the following error: ".to_owned() + stderr_string
            };

            return Err(HttmError::new(&msg).into());
        }

        let stdout_string = std::str::from_utf8(&process_output.stdout)?.trim();

        if stdout_string.is_empty() {
            let msg = "No difference between the snap name given and the present state of the filesystem.  Quitting.";

            return Err(HttmError::new(msg).into());
        }

        Ok(stdout_string.to_owned())
    }

    fn exec_snap(
        zfs_command: &Path,
        dataset_name: &str,
        snap_name: &str,
        snap_type: PrecautionarySnapType,
    ) -> HttmResult<()> {
        let mut process_args = vec!["snapshot".to_owned()];

        match snap_type {
            PrecautionarySnapType::Pre => {
                // all snapshots should have the same timestamp
                let timestamp = get_date(
                    GLOBAL_CONFIG.requested_utc_offset,
                    &SystemTime::now(),
                    DateFormat::Timestamp,
                );

                let snap_name = format!(
                    "{}@snap_pre_{}_httmSnapRollForward",
                    dataset_name, timestamp
                );

                process_args.push(snap_name);
            }
            PrecautionarySnapType::Post(_original_snap_name) => {
                let snap_name = format!(
                    "{}@snap_post_{}_httmSnapRollForward",
                    dataset_name, snap_name
                );

                process_args.push(snap_name);
            }
        }

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
            let output_buf = format!("httm took a snapshot named: {}\n", &snap_name);

            print_output_buf(output_buf)
        }
    }
}

enum DiffType {
    Removed,
    Created,
    Modified,
    // zfs diff semantics are: old file name -> new file name
    // old file name will be the key, and new file name will be stored in the value
    Renamed(PathBuf),
}

#[allow(dead_code)]
struct DiffMap {
    inner: BTreeMap<PathData, DiffType>,
    dataset_name: String,
    snap_name: String,
}

impl DiffMap {
    fn new(zfs_diff_str: &str, dataset_name: &str, snap_name: &str) -> HttmResult<Self> {
        let diff_map: BTreeMap<PathData, DiffType> =
            zfs_diff_str
                .par_lines()
                .filter_map(|line| {
                    let split_line: Vec<&str> = line.split('\t').collect();

                    match split_line.first() {
                        Some(elem) if elem == &"-" => split_line.get(1).map(|path_string| {
                            (PathData::from(Path::new(path_string)), DiffType::Removed)
                        }),
                        Some(elem) if elem == &"+" => split_line.get(1).map(|path_string| {
                            (PathData::from(Path::new(path_string)), DiffType::Created)
                        }),
                        Some(elem) if elem == &"M" => split_line.get(1).map(|path_string| {
                            (PathData::from(Path::new(path_string)), DiffType::Modified)
                        }),
                        Some(elem) if elem == &"R" => split_line.get(1).map(|path_string| {
                            (
                                PathData::from(Path::new(path_string)),
                                DiffType::Renamed(PathBuf::from(split_line.get(2).expect(
                                    "diff of type rename did not contain a new name value",
                                ))),
                            )
                        }),
                        _ => None,
                    }
                })
                .collect();

        if diff_map.is_empty() {
            let msg = "httm was unable to parse the output of 'zfs diff'.  Quitting.";

            return Err(HttmError::new(msg).into());
        }

        Ok(DiffMap {
            inner: diff_map,
            dataset_name: dataset_name.to_owned(),
            snap_name: snap_name.to_owned(),
        })
    }

    fn roll_forward(&self) -> HttmResult<()> {
        let snaps_selected_for_search = &[SnapDatasetType::MostProximate];

        self.inner
            .par_iter()
            .try_for_each(|(pathdata, diff_type)| {
                let all_versions: Vec<PathData> = VersionsMap::get_search_bundles(pathdata, snaps_selected_for_search)
                    .flat_map(|search_bundle| search_bundle.get_versions_processed(&ListSnapsOfType::All))
                    .collect();

                match diff_type {
                    DiffType::Removed => {
                        if let Some(snap_file) = self.find_snap_version(&all_versions) {
                            copy_recursive(&snap_file, &pathdata.path_buf, true)?;
                            println!("Removed File: httm moved {:?} back to its original location: {:?}.", &pathdata.path_buf, snap_file);
                        } else {
                            eprintln!("WARNING: snap_file does not exist")
                        }
                    }
                    DiffType::Created => {
                        if pathdata.path_buf.is_dir() {
                            std::fs::remove_dir_all(&pathdata.path_buf)?;
                        } else {
                            std::fs::remove_file(&pathdata.path_buf)?;
                        }
                        println!("Created File: httm deleted {:?}, a newly created file.", &pathdata.path_buf);
                    }
                    DiffType::Modified => {
                        if let Some(snap_file) = self.find_snap_version(&all_versions) {
                            copy_recursive(&snap_file, &pathdata.path_buf, true)?;
                            println!("Modified File: httm has overwritten {:?} with the file contents from a snapshot: {:?}.", &pathdata.path_buf, snap_file);
                        } else {
                            eprintln!("WARNING: snap_file does not exist")
                        }
                    }
                    DiffType::Renamed(new_file_name) => {
                        copy_recursive(new_file_name, &pathdata.path_buf, true)?;
                        println!("Renamed File: httm moved {:?} back to its original location: {:?}.", new_file_name, &pathdata.path_buf);
                    }
                }

                Ok(())
            })
    }

    fn find_snap_version(&self, all_versions: &[PathData]) -> Option<PathBuf> {
        let os_string = OsStr::new(&self.snap_name);

        all_versions
            .iter()
            .find(|pathdata| {
                pathdata
                    .path_buf
                    .components()
                    .any(|component| component.as_os_str() == os_string)
            })
            .map(|pathdata| pathdata.path_buf.clone())
    }
}
