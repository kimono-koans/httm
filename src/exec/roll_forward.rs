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

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command as ExecProcess;
use std::process::Stdio;
use std::time::SystemTime;

use which::which;

use crate::data::paths::PathData;
use crate::library::diff_copy::diff_copy;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{copy_attributes, remove_recursive};
use crate::library::utility::{get_date, is_metadata_different, DateFormat};
use crate::print_output_buf;
use crate::GLOBAL_CONFIG;

pub enum PrecautionarySnapType {
    Pre,
    Post,
}

#[derive(Clone)]
enum DiffType {
    Removed,
    Created,
    Modified,
    // zfs diff semantics are: old file name -> new file name
    // old file name will be the key, and new file name will be stored in the value
    Renamed(PathBuf),
}

pub struct RollForward;

impl RollForward {
    pub fn exec(full_snap_name: &str) -> HttmResult<()> {
        if !nix::unistd::geteuid().is_root() {
            return Err(HttmError::new(
                "Superuser privileges are require to execute a roll forward.",
            )
            .into());
        }

        let zfs_command = if let Ok(zfs_command) = which("zfs") {
            zfs_command
        } else {
            return Err(HttmError::new(
                "'zfs' command not found. Make sure the command 'zfs' is in your path.",
            )
            .into());
        };

        let (dataset_name, snap_name) = if let Some(res) = full_snap_name.split_once('@') {
            res
        } else {
            let msg = format!("{} is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.", full_snap_name);
            return Err(HttmError::new(&msg).into());
        };

        let mut process_handle = Self::exec_diff(full_snap_name, &zfs_command)?;

        let mut stream = Self::ingest(&mut process_handle)?;

        let pre_exec_snap_name = RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Pre,
        )?;

        match Self::roll_forward(&mut stream, snap_name, dataset_name) {
            Ok(_) => {
                println!("httm roll forward completed successfully.");
            }
            Err(err) => {
                let msg = format!(
                    "httm roll forward failed for the following reason: {}.\n\
                Attempting roll back to precautionary pre-execution snapshot.",
                    err
                );
                eprintln!("{}", msg);

                Self::exec_rollback(&pre_exec_snap_name, &zfs_command)
                    .map(|_| println!("Rollback succeeded."))?;

                std::process::exit(1)
            }
        };

        RollForward::exec_snap(
            &zfs_command,
            dataset_name,
            snap_name,
            PrecautionarySnapType::Post,
        )
        .map(|_res| ())
    }

    fn exec_rollback(pre_exec_snap_name: &str, zfs_command: &Path) -> HttmResult<()> {
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

    fn exec_diff(full_snapshot_name: &str, zfs_command: &Path) -> HttmResult<Child> {
        let mut process_args = vec!["diff", "-H"];
        process_args.push(full_snapshot_name);

        let process_handle = ExecProcess::new(zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .spawn()?;

        Ok(process_handle)
    }

    fn exec_snap(
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

    fn check_stderr(process_handle: &mut Child) {
        if process_handle.stderr.is_some() {
            let mut stderr_buffer = std::io::BufReader::new(process_handle.stderr.take().unwrap());

            if stderr_buffer.fill_buf().map(|b| !b.is_empty()).unwrap() {
                let buffer = stderr_buffer.fill_buf().unwrap().to_vec();
                let output_buf = std::str::from_utf8(&buffer).unwrap();
                eprintln!("Error: {}", output_buf);
                std::process::exit(1);
            }
        }
    }

    fn ingest(
        process_handle: &mut Child,
    ) -> HttmResult<impl Iterator<Item = (PathData, DiffType)> + '_> {
        let stdout_buffer = if let Some(output) = process_handle.stdout.take() {
            std::io::BufReader::new(output)
        } else {
            Self::check_stderr(process_handle);

            println!("'zfs diff' did not appear to contain any modified files.  Quitting.");
            std::process::exit(0);
        };

        let iterator =
            stdout_buffer
                .lines()
                .filter_map(|line| line.ok())
                .filter_map(move |line| {
                    let split_line: Vec<&str> = line.split('\t').collect();

                    Self::check_stderr(process_handle);

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
                });

        Ok(iterator)
    }

    fn roll_forward<I>(stream: I, snap_name: &str, _dataset_name: &str) -> HttmResult<()>
    where
        I: Iterator<Item = (PathData, DiffType)>,
    {
        stream
            .filter_map(|(pathdata, diff_type)| {
                pathdata
                    .get_proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)
                    .ok()
                    .and_then(|proximate_dataset_mount| {
                        pathdata
                            .get_relative_path(proximate_dataset_mount)
                            .ok()
                            .map(|relative_path| {
                                let snap_file_path: PathBuf = [
                                    proximate_dataset_mount,
                                    Path::new(".zfs/snapshot"),
                                    Path::new(&snap_name),
                                    relative_path,
                                ]
                                .iter()
                                .collect();

                                (pathdata.to_owned(), diff_type.clone(), snap_file_path)
                            })
                    })
            })
            .try_for_each(|(pathdata, diff_type, snap_file_path)| {
                let snap_file = PathData::from(snap_file_path.as_path());

                match diff_type {
                    DiffType::Removed => {
                        if let Err(err) = Self::copy_direct(&snap_file.path_buf, &pathdata.path_buf)
                        {
                            eprintln!("{}", err);
                            let msg = format!(
                                "WARNING: could not overwrite {:?} with snapshot file version {:?}",
                                &pathdata.path_buf, snap_file.path_buf
                            );
                            return Err(HttmError::new(&msg).into());
                        }
                        is_metadata_different(&snap_file.path_buf, &pathdata.path_buf)
                    }
                    DiffType::Created => match remove_recursive(&pathdata.path_buf) {
                        Ok(_) => {
                            if pathdata.path_buf.exists() {
                                let msg = format!(
                                    "WARNING: File should not exist after deletion {:?}",
                                    pathdata.path_buf
                                );
                                return Err(HttmError::new(&msg).into());
                            }
                            Ok(())
                        }
                        Err(err) => {
                            eprintln!("{}", err);
                            let msg = format!("WARNING: Removal of file {:?} failed", err);
                            Err(HttmError::new(&msg).into())
                        }
                    },
                    DiffType::Modified => {
                        if let Err(err) = Self::copy_direct(&snap_file.path_buf, &pathdata.path_buf)
                        {
                            eprintln!("{}", err);
                            let msg = format!(
                                "WARNING: could not overwrite {:?} with snapshot file version {:?}",
                                &pathdata.path_buf, snap_file.path_buf
                            );
                            return Err(HttmError::new(&msg).into());
                        }
                        is_metadata_different(&snap_file.path_buf, &pathdata.path_buf)
                    }
                    DiffType::Renamed(new_file_name) => {
                        if let Err(err) = std::fs::rename(&new_file_name, &pathdata.path_buf) {
                            eprintln!("{}", err);
                            let msg = format!(
                                "WARNING: could not rename {:?} to {:?}",
                                new_file_name, &pathdata.path_buf
                            );
                            return Err(HttmError::new(&msg).into());
                        }

                        is_metadata_different(&snap_file.path_buf, &pathdata.path_buf)
                    }
                }
            })
    }

    // why include here? because I think this only works with the correct semantics
    // that is -- output from zfs diff,
    pub fn copy_direct(src: &Path, dst: &Path) -> HttmResult<()> {
        if src.is_dir() {
            if !dst.exists() {
                std::fs::create_dir_all(dst)?;
            }
            assert!(dst.exists())
        } else {
            // create parent for file to land
            {
                let src_parent = src.parent().unwrap();

                let dst_parent = if let Some(parent) = dst.parent() {
                    parent.to_path_buf()
                } else {
                    let mut parent = dst.to_path_buf();
                    parent.pop();
                    parent
                };

                if !dst_parent.exists() {
                    std::fs::create_dir_all(&dst_parent)?;
                }

                copy_attributes(src_parent, &dst_parent)?;
                assert!(dst_parent.exists())
            }

            if src.is_symlink() {
                let link_target = std::fs::read_link(src)?;
                std::os::unix::fs::symlink(link_target, dst)?
            } else {
                diff_copy(src, dst)?;
            }
        }

        copy_attributes(src, dst)
    }
}
