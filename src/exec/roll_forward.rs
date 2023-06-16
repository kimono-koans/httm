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

use std::cmp::Ordering;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command as ExecProcess;
use std::process::Stdio;

use nu_ansi_term::Color::{Blue, Red};
use once_cell::sync::OnceCell;
use which::which;

use crate::data::paths::PathData;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::{PrecautionarySnapType, SnapGuard};
use crate::library::utility::{copy_direct, remove_recursive};
use crate::library::utility::{is_metadata_different, user_has_effective_root};
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};

#[derive(Debug, Clone)]
struct DiffEvent {
    pathdata: PathData,
    diff_type: DiffType,
    time: DiffTime,
}

impl DiffEvent {
    fn new(path_string: &str, diff_type: DiffType, time_str: &str) -> Self {
        Self {
            pathdata: PathData::from(Path::new(path_string)),
            diff_type,
            time: DiffTime::new(time_str).expect("Could not parse a zfs diff time value."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffTime {
    secs: u64,
    nanos: u64,
}

impl DiffTime {
    fn new(time_str: &str) -> Option<Self> {
        let (secs, nanos) = time_str.split_once('.')?;

        let time = DiffTime {
            secs: secs.parse::<u64>().ok()?,
            nanos: nanos.parse::<u64>().ok()?,
        };

        Some(time)
    }
}

impl std::cmp::Ord for DiffTime {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let secs_ordering = self.secs.cmp(&other.secs);

        if secs_ordering.is_eq() {
            return self.nanos.cmp(&other.nanos);
        }

        secs_ordering
    }
}

impl std::cmp::PartialOrd for DiffTime {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
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
        user_has_effective_root()?;

        let (dataset_name, snap_name) = if let Some(res) = full_snap_name.split_once('@') {
            res
        } else {
            let msg = format!("{} is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.", full_snap_name);
            return Err(HttmError::new(&msg).into());
        };

        let snap_guard: SnapGuard =
            SnapGuard::new(dataset_name, PrecautionarySnapType::PreRollForward)?;

        let mut process_handle = Self::exec_diff(full_snap_name)?;

        let mut stream = Self::ingest(&mut process_handle)?;

        match Self::roll_forward(&mut stream, snap_name) {
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

                snap_guard
                    .rollback()
                    .map(|_| println!("Rollback succeeded."))?;

                std::process::exit(1)
            }
        };

        SnapGuard::new(
            dataset_name,
            PrecautionarySnapType::PostRollForward(snap_name.to_owned()),
        )
        .map(|_res| ())
    }

    fn exec_diff(full_snapshot_name: &str) -> HttmResult<Child> {
        let zfs_command = which("zfs").map_err(|_err| {
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
        })?;

        // -H: tab separated, -t: Specify time, -h: Normalize paths (don't use escape codes)
        let process_args = vec!["diff", "-H", "-t", "-h", full_snapshot_name];

        let process_handle = ExecProcess::new(zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(process_handle)
    }

    fn ingest(process_handle: &mut Child) -> HttmResult<impl Iterator<Item = DiffEvent> + '_> {
        if let Some(output) = process_handle.stdout.take() {
            let stdout_buffer = std::io::BufReader::new(output);

            let ret = stdout_buffer
                .lines()
                .map(|line| line.expect("Could not obtain line from string."))
                .filter_map(move |line| {
                    let split_line: Vec<&str> = line.split('\t').collect();

                    let time_str = split_line
                        .first()
                        .expect("Could not obtain a timestamp for diff event.");

                    let res = match split_line.get(1) {
                        Some(event) if event == &"-" => split_line.get(2).map(|path_string| {
                            DiffEvent::new(path_string, DiffType::Removed, time_str)
                        }),
                        Some(event) if event == &"+" => split_line.get(2).map(|path_string| {
                            DiffEvent::new(path_string, DiffType::Created, time_str)
                        }),
                        Some(event) if event == &"M" => split_line.get(2).map(|path_string| {
                            DiffEvent::new(path_string, DiffType::Modified, time_str)
                        }),
                        Some(event) if event == &"R" => split_line.get(2).map(|path_string| {
                            let new_file_name =
                                PathBuf::from(split_line.get(3).expect(
                                    "diff of type rename did not contain a new name value",
                                ));
                            DiffEvent::new(path_string, DiffType::Renamed(new_file_name), time_str)
                        }),
                        _ => panic!("Could not parse diff event."),
                    };

                    res
                });

            Ok(ret)
        } else {
            Err(HttmError::new("'zfs diff' reported no changes to dataset").into())
        }
    }

    fn roll_forward<I>(stream: I, snap_name: &str) -> HttmResult<()>
    where
        I: Iterator<Item = DiffEvent>,
    {
        let cell: OnceCell<PathBuf> = OnceCell::new();

        let mut iter_peekable = stream.peekable();

        if iter_peekable.peek().is_none() {
            return Err(HttmError::new("'zfs diff' reported no changes to dataset").into());
        }

        // zfs-diff can return multiple file actions for a single inode, here we dedup
        let mut group_map: Vec<(PathBuf, Vec<DiffEvent>)> = iter_peekable
            .into_group_map_by(|event| event.pathdata.path_buf.clone())
            .into_iter()
            .collect();

        // now sort by number of components, want to build from the bottom up, do less dir creation, etc.
        group_map.sort_by_key(|(key, _values)| key.components().count());

        // into iter and reverse because we want to go smallest first
        group_map
            .into_iter()
            .map(|(_key, mut values)| {
                values.sort_by_key(|event| event.time.clone());
                values.pop()
            })
            .flatten()
            .map(|event| {
                let proximate_dataset_mount = cell.get_or_init(|| {
                    event
                        .pathdata
                        .proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)
                        .expect("Could not obtain proximate dataset mount.")
                        .to_owned()
                });

                let snap_file_path =
                    Self::snap_path(&event.pathdata, snap_name, proximate_dataset_mount)
                        .expect("Could not obtain snap file path for live version.");

                (event, snap_file_path)
            })
            .try_for_each(|(event, snap_file_path)| Self::diff_action(&event, &snap_file_path))
    }

    fn snap_path(
        event_pathdata: &PathData,
        snap_name: &str,
        proximate_dataset_mount: &Path,
    ) -> Option<PathBuf> {
        event_pathdata
            .relative_path(proximate_dataset_mount)
            .ok()
            .map(|relative_path| {
                let snap_file_path: PathBuf = [
                    proximate_dataset_mount,
                    Path::new(ZFS_SNAPSHOT_DIRECTORY),
                    Path::new(&snap_name),
                    relative_path,
                ]
                .iter()
                .collect();

                snap_file_path
            })
    }

    fn diff_action(event: &DiffEvent, snap_file_path: &Path) -> HttmResult<()> {
        let snap_file = PathData::from(snap_file_path);

        // zfs-diff can return multiple file actions for a single inode
        // since we exclude older file actions, if rename or created is the last action,
        // we should make sure it has the latest data, so a simple rename is not enough
        // this is internal to the fn Self::remove()
        match &event.diff_type {
            DiffType::Removed | DiffType::Modified => {
                Self::copy(&snap_file.path_buf, &event.pathdata.path_buf)
            }
            DiffType::Created => Self::remove(&snap_file.path_buf, &event.pathdata.path_buf),
            DiffType::Renamed(new_file_name) => Self::remove(&snap_file.path_buf, &new_file_name),
        }
    }

    fn copy(src: &Path, dst: &Path) -> HttmResult<()> {
        if let Err(err) = copy_direct(src, dst, true) {
            eprintln!("Error: {}", err);
            let msg = format!(
                "WARNING: could not overwrite {:?} with snapshot file version {:?}",
                dst, src
            );
            return Err(HttmError::new(&msg).into());
        }

        is_metadata_different(src, dst)?;
        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    fn remove(src: &Path, dst: &Path) -> HttmResult<()> {
        if src.exists() {
            return Self::copy(src, dst);
        }

        match remove_recursive(dst) {
            Ok(_) => {
                if dst.exists() {
                    let msg = format!("WARNING: File should not exist after deletion {:?}", dst);
                    return Err(HttmError::new(&msg).into());
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                let msg = format!("WARNING: Could not delete file {:?}", dst);
                return Err(HttmError::new(&msg).into());
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Red.paint("Removed  "), dst);

        Ok(())
    }
}
