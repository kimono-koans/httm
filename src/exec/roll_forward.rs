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
use crate::library::utility::{copy_recursive, remove_recursive};
use crate::library::utility::{get_date, DateFormat};
use crate::lookup::versions::{SnapDatasetType, VersionsMap};
use crate::print_output_buf;
use crate::GLOBAL_CONFIG;

pub enum PrecautionarySnapType {
    Pre,
    Post,
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

        let (dataset_name, snap_name) = full_snap_name.split_once('@').expect(
            "A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.",
        );

        let zfs_diff_str = Self::exec_diff(full_snap_name, &zfs_command)?;

        let diff_map = DiffMap::new(&zfs_diff_str, dataset_name, snap_name)?;

        RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Pre,
        )?;

        if diff_map.roll_forward().is_ok() {
            println!("httm roll forward completed successfully.")
        } else {
            eprintln!(
                "httm roll forward failed.  Rolling back to precautionary pre-execution snapshot."
            )
        };

        RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Post,
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

        let new_snap_name = match &snap_type {
            PrecautionarySnapType::Pre => {
                // all snapshots should have the same timestamp
                let timestamp = get_date(
                    GLOBAL_CONFIG.requested_utc_offset,
                    &SystemTime::now(),
                    DateFormat::Timestamp,
                );

                let new_snap_name = format!(
                    "{}@snap_pre_{}_httmSnapRollForward",
                    dataset_name, timestamp
                );

                new_snap_name
            }
            PrecautionarySnapType::Post => {
                let new_snap_name = format!(
                    "{}@snap_post_{}_httmSnapRollForward",
                    dataset_name, snap_name
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
                    format!("httm took a pre-execution snapshot named: {}\n", &new_snap_name)
                }
                PrecautionarySnapType::Post => {
                    format!(
                        "httm took a post-execution snapshot named: {}\n",
                        &new_snap_name
                    )
                }
            };

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
            .iter()
            .for_each(|(pathdata, diff_type)| {
                let all_versions: Vec<PathData> = VersionsMap::get_search_bundles(pathdata, snaps_selected_for_search)
                    .flat_map(|search_bundle| search_bundle.get_versions_processed(&ListSnapsOfType::All))
                    .collect();

                match diff_type {
                    DiffType::Removed => {
                        if let Some(snap_file) = self.find_snap_version(&all_versions) {
                            if copy_recursive(&snap_file.path_buf, &pathdata.path_buf, true).is_ok() {
                                if GLOBAL_CONFIG.opt_debug {
                                    println!("Removed File: httm moved {:?} back to its original location: {:?}.", &pathdata.path_buf, snap_file);
                                }

                                if pathdata.get_md_infallible() != snap_file.get_md_infallible() {
                                    eprintln!("WARNING: Metadata mismatch: {:?} !-> {:?}", snap_file, &pathdata.path_buf)
                                }
                            } else {
                                eprintln!("WARNING: could not overwrite {:?} with snapshot file version {:?}", &pathdata.path_buf, snap_file)
                            }
                        } else {
                            eprintln!("WARNING: Snapshot file path for {:?} could not be found.", pathdata.path_buf)
                        }
                    }
                    DiffType::Created => {
                        if pathdata.path_buf.exists() && remove_recursive(&pathdata.path_buf).is_ok() && GLOBAL_CONFIG.opt_debug {
                            println!("Created File: httm deleted {:?}, a newly created file.", &pathdata.path_buf);
                        }

                        if pathdata.path_buf.exists() {
                            eprintln!("WARNING: File should not exist {:?}", &pathdata.path_buf)
                        }
                    }
                    DiffType::Modified => {
                        if let Some(snap_file) = self.find_snap_version(&all_versions) {
                            if copy_recursive(&snap_file.path_buf, &pathdata.path_buf, true).is_ok() {
                                if GLOBAL_CONFIG.opt_debug {
                                    println!("Modified File: httm has overwritten {:?} with the file contents from a snapshot: {:?}.", &pathdata.path_buf, snap_file);
                                }

                                if pathdata.get_md_infallible() != snap_file.get_md_infallible() {
                                    eprintln!("WARNING: Metadata mismatch: {:?} !-> {:?}", snap_file, &pathdata.path_buf)
                                }
                            } else {
                                eprintln!("WARNING: could not overwrite {:?} with snapshot file version {:?}", &pathdata.path_buf, snap_file)
                            }
                        } else {
                            eprintln!("WARNING: Snapshot file path for {:?} could not be found.", pathdata.path_buf)
                        }
                    }
                    DiffType::Renamed(new_file_name) => {
                        if copy_recursive(new_file_name, &pathdata.path_buf, true).is_ok() {
                            if GLOBAL_CONFIG.opt_debug {
                                println!("Renamed File: httm moved {:?} back to its original location: {:?}.", new_file_name, &pathdata.path_buf);
                            }

                            if pathdata.get_md_infallible() != PathData::from(new_file_name.as_path()).get_md_infallible() {
                                eprintln!("WARNING: Metadata mismatch: {:?} !-> {:?}", new_file_name, &pathdata.path_buf)
                            }
                        } else {
                            eprintln!("WARNING: could not overwrite {:?} with renamed file version {:?}", &pathdata.path_buf, new_file_name)
                        }
                    }
                }
            });

        Ok(())
    }

    fn find_snap_version(&self, all_versions: &[PathData]) -> Option<PathData> {
        let snap_name_string = OsStr::new(&self.snap_name);

        all_versions
            .par_iter()
            .find_first(|pathdata| {
                pathdata
                    .path_buf
                    .components()
                    .any(|component| component.as_os_str() == snap_name_string)
            })
            .map(|pathdata| pathdata.to_owned())
    }
}
