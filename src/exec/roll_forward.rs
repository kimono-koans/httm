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
use std::fs::{read_dir, remove_file};
use std::io::{BufRead, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::process::Stdio;
use std::process::{Child, ChildStdout};
use std::thread::JoinHandle;

use hashbrown::{HashMap, HashSet};
use nu_ansi_term::Color::{Blue, Green, Red, Yellow};
use rayon::prelude::*;
use which::which;

use crate::config::generate::RollForwardConfig;
use crate::data::paths::BasicDirEntryInfo;
use crate::data::paths::PathData;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::{PrecautionarySnapType, SnapGuard};
use crate::library::utility::{copy_attributes, copy_direct, remove_recursive};
use crate::library::utility::{is_metadata_same, user_has_effective_root};
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};

#[derive(Debug, Clone)]
struct DiffEvent {
    path_buf: PathBuf,
    diff_type: DiffType,
    time: DiffTime,
}

impl DiffEvent {
    fn new(path_string: &str, diff_type: DiffType, time_str: &str) -> Self {
        let path_buf = PathBuf::from(&path_string);

        Self {
            path_buf,
            diff_type,
            time: DiffTime::new(time_str).expect("Could not parse a zfs diff time value."),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
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

pub struct RollForward {
    dataset_name: String,
    snap_name: String,
    roll_config: RollForwardConfig,
    proximate_dataset_mount: PathBuf,
}

impl RollForward {
    pub fn new(roll_config: RollForwardConfig) -> HttmResult<Self> {
        let (dataset_name, snap_name) = if let Some(res) =
            roll_config.full_snap_name.split_once('@')
        {
            res
        } else {
            let msg = format!("{} is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.", roll_config.full_snap_name);
            return Err(HttmError::new(&msg).into());
        };

        let proximate_dataset_mount = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .iter()
            .find(|(_mount, md)| md.source == PathBuf::from(&dataset_name))
            .map(|(mount, _)| mount.to_owned())
            .ok_or_else(|| HttmError::new("Could not determine proximate dataset mount"))?;

        Ok(Self {
            dataset_name: dataset_name.to_string(),
            snap_name: snap_name.to_string(),
            roll_config,
            proximate_dataset_mount,
        })
    }

    pub fn exec(&self) -> HttmResult<()> {
        user_has_effective_root()?;

        let snap_guard: SnapGuard =
            SnapGuard::new(&self.dataset_name, PrecautionarySnapType::PreRollForward)?;

        match self.roll_forward() {
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
            &self.dataset_name,
            PrecautionarySnapType::PostRollForward(self.snap_name.to_owned()),
        )
        .map(|_res| ())
    }

    fn zfs_diff_cmd(&self) -> HttmResult<Child> {
        let zfs_command = which("zfs").map_err(|_err| {
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
        })?;

        // -H: tab separated, -t: Specify time, -h: Normalize paths (don't use escape codes)
        let process_args = vec!["diff", "-H", "-t", "-h", &self.roll_config.full_snap_name];

        let process_handle = ExecProcess::new(zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(process_handle)
    }

    fn ingest(
        process_handle: &mut Option<ChildStdout>,
    ) -> HttmResult<impl Iterator<Item = DiffEvent> + '_> {
        const IN_BUFFER_SIZE: usize = 65_536;

        if let Some(output) = process_handle {
            let stdout_buffer = std::io::BufReader::with_capacity(IN_BUFFER_SIZE, output);

            let ret = stdout_buffer
                .lines()
                .map(|line| line.expect("Could not obtain line from string."))
                .filter_map(move |line| {
                    let split_line: Vec<&str> = line.split('\t').collect();

                    let time_str = split_line
                        .first()
                        .expect("Could not obtain a timestamp for diff event.");

                    match split_line.get(1) {
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
                    }
                });

            Ok(ret)
        } else {
            Err(HttmError::new("'zfs diff' reported no changes to dataset").into())
        }
    }

    fn spawn_preserve_links(
        &self,
    ) -> (
        JoinHandle<HttmResult<HardLinkMap>>,
        JoinHandle<HttmResult<HardLinkMap>>,
    ) {
        let snap_dataset = self.snap_dataset();

        let proximate_dataset_mount = self.proximate_dataset_mount.clone();

        let snap_handle = std::thread::spawn(move || HardLinkMap::new(&snap_dataset));
        let live_handle = std::thread::spawn(move || HardLinkMap::new(&proximate_dataset_mount));

        (snap_handle, live_handle)
    }

    fn roll_forward(&self) -> HttmResult<()> {
        let (snap_handle, live_handle) = self.spawn_preserve_links();

        let mut process_handle = self.zfs_diff_cmd()?;

        let opt_stderr = process_handle.stderr.take();
        let mut opt_stdout = process_handle.stdout.take();

        let stream = Self::ingest(&mut opt_stdout)?;

        let mut iter_peekable = stream.peekable();

        if iter_peekable.peek().is_none() {
            return Err(HttmError::new("'zfs diff' reported no changes to dataset").into());
        }

        // zfs-diff can return multiple file actions for a single inode, here we dedup
        eprintln!("Building a map of ZFS filesystem events since the specified snapshot:");
        let mut group_map: Vec<(PathBuf, Vec<DiffEvent>)> = iter_peekable
            .map(|event| {
                self.roll_config.progress_bar.tick();
                event
            })
            .into_group_map_by(|event| event.path_buf.clone())
            .into_iter()
            .collect();

        if let Some(mut stderr) = opt_stderr {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf)?;

            if !buf.is_empty() {
                let msg = format!("'zfs diff' command reported an error: {}", buf);
                return Err(HttmError::new(&msg).into());
            }
        }

        // now sort by number of components, want to build from the bottom up, do less dir creation, etc.
        group_map.par_sort_unstable_by_key(|(key, _values)| key.components().count());

        // need to wait for these to finish before executing any diff_action
        let snap_map = snap_handle
            .join()
            .map_err(|_err| HttmError::new("Thread panicked!"))??;

        let live_map = live_handle
            .join()
            .map_err(|_err| HttmError::new("Thread panicked!"))??;

        let preserve_hard_links = PreserveHardLinks::new(&live_map, &snap_map, self.to_owned())?;

        preserve_hard_links.preserve_orphans()?;

        // into iter and reverse because we want to go largest first
        group_map
            .into_par_iter()
            .rev()
            .flat_map(|(_key, mut values)| {
                values.sort_by_key(|event| event.time);
                values.pop()
            })
            .filter(|event| event.path_buf.exists())
            .map(|event| {
                let snap_file_path = self
                    .snap_path(&event.path_buf)
                    .expect("Could not obtain snap file path for live version.");

                (event, snap_file_path)
            })
            .try_for_each(|(event, snap_file_path)| self.diff_action(&event, &snap_file_path))?;

        preserve_hard_links.preserve_live_links()?;
        preserve_hard_links.preserve_snap_links()
    }

    fn snap_path(&self, path: &Path) -> Option<PathBuf> {
        PathData::from(path)
            .relative_path(&self.proximate_dataset_mount)
            .ok()
            .map(|relative_path| {
                let snap_file_path: PathBuf = [
                    self.proximate_dataset_mount.as_path(),
                    Path::new(ZFS_SNAPSHOT_DIRECTORY),
                    Path::new(&self.snap_name),
                    relative_path,
                ]
                .iter()
                .collect();

                snap_file_path
            })
    }

    fn diff_action(&self, event: &DiffEvent, snap_file_path: &Path) -> HttmResult<()> {
        // zfs-diff can return multiple file actions for a single inode
        // since we exclude older file actions, if rename or created is the last action,
        // we should make sure it has the latest data, so a simple rename is not enough
        // this is internal to the fn Self::remove()
        match &event.diff_type {
            DiffType::Removed | DiffType::Modified => Self::copy(&snap_file_path, &event.path_buf),
            DiffType::Created => Self::overwrite_or_remove(&snap_file_path, &event.path_buf),
            DiffType::Renamed(new_file_name) => {
                Self::overwrite_or_remove(&snap_file_path, new_file_name)
            }
        }
    }

    fn copy(src: &Path, dst: &Path) -> HttmResult<()> {
        if let Err(err) = copy_direct(src, dst, true) {
            eprintln!("Error: {}", err);
            let msg = format!(
                "Could not overwrite {:?} with snapshot file version {:?}",
                dst, src
            );
            return Err(HttmError::new(&msg).into());
        }

        is_metadata_same(src, dst)?;
        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    fn snap_dataset(&self) -> PathBuf {
        [
            self.proximate_dataset_mount.as_path(),
            Path::new(ZFS_SNAPSHOT_DIRECTORY),
            Path::new(&self.snap_name),
        ]
        .iter()
        .collect()
    }

    fn overwrite_or_remove(src: &Path, dst: &Path) -> HttmResult<()> {
        // overwrite
        if src.exists() {
            return Self::copy(src, dst);
        }

        // or remove
        Self::remove(dst)
    }

    fn remove(dst: &Path) -> HttmResult<()> {
        // overwrite
        if !dst.exists() {
            return Ok(());
        }

        match remove_recursive(dst) {
            Ok(_) => {
                if dst.exists() {
                    let msg = format!("File should not exist after deletion {:?}", dst);
                    return Err(HttmError::new(&msg).into());
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                let msg = format!("Could not delete file {:?}", dst);
                return Err(HttmError::new(&msg).into());
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Red.paint("Removed  "), dst);

        Ok(())
    }
}

// key: inode, values: Paths
struct HardLinkMap {
    link_map: HashMap<u64, Vec<BasicDirEntryInfo>>,
    remainder: HashSet<PathBuf>,
}

impl HardLinkMap {
    fn new(requested_path: &Path) -> HttmResult<Self> {
        let constructed = BasicDirEntryInfo {
            path: requested_path.to_path_buf(),
            file_type: None,
        };

        let mut queue: Vec<BasicDirEntryInfo> = vec![constructed];
        let mut tmp: HashMap<u64, Vec<BasicDirEntryInfo>> = HashMap::new();

        // condition kills iter when user has made a selection
        // pop_back makes this a LIFO queue which is supposedly better for caches
        while let Some(item) = queue.pop() {
            // no errors will be propagated in recursive mode
            // far too likely to run into a dir we don't have permissions to view
            let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
                read_dir(item.path)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry))
                    .partition(|dir_entry| dir_entry.path.is_dir());

            let mut combined = vec_files;
            combined.extend_from_slice(&vec_dirs);
            queue.extend_from_slice(&vec_dirs);

            combined
                .into_iter()
                .filter(|entry| {
                    if let Some(ft) = entry.file_type {
                        return ft.is_file() || {
                            ft.is_dir()
                                && read_dir(&entry.path)
                                    .map(|mut read_dir| read_dir.next().is_none())
                                    .unwrap_or(false)
                        };
                    }

                    false
                })
                .filter_map(|entry| entry.path.metadata().ok().map(|md| (md.ino(), entry)))
                .for_each(|(ino, entry)| match tmp.get_mut(&ino) {
                    Some(values) => values.push(entry),
                    None => {
                        let _ = tmp.insert(ino, vec![entry]);
                    }
                });
        }

        let (link_map, remain_tmp): (
            HashMap<u64, Vec<BasicDirEntryInfo>>,
            HashMap<u64, Vec<BasicDirEntryInfo>>,
        ) = tmp.into_iter().partition(|(_ino, values)| {
            values.len() > 1 && !values[0].file_type.map(|ft| ft.is_dir()).unwrap_or(false)
        });

        let remainder = remain_tmp
            .into_iter()
            .flat_map(|(_ino, vec)| vec.into_iter().map(|entry: BasicDirEntryInfo| entry.path))
            .collect();

        Ok(Self {
            link_map,
            remainder,
        })
    }
}

struct PreserveHardLinks<'a> {
    live_map: &'a HardLinkMap,
    snap_map: &'a HardLinkMap,
    roll_forward: &'a RollForward,
}

impl<'a> PreserveHardLinks<'a> {
    fn new(
        live_map: &'a HardLinkMap,
        snap_map: &'a HardLinkMap,
        roll_forward: &'a RollForward,
    ) -> HttmResult<Self> {
        Ok(Self {
            live_map,
            snap_map,
            roll_forward,
        })
    }

    fn preserve_live_links(&self) -> HttmResult<()> {
        let mut none_removed: bool = true;

        self.live_map
            .link_map
            .iter()
            .try_for_each(|(_key, values)| {
                values
                    .iter()
                    .map(|live_path| {
                        let snap_path = self
                            .roll_forward
                            .snap_path(&live_path.path)
                            .expect("Could not obtain snap path for live path");

                        (live_path, snap_path)
                    })
                    .filter(|(_live_path, snap_path)| !snap_path.exists())
                    .try_for_each(|(live_path, _snap_path)| {
                        none_removed = false;
                        Self::rm_hard_link(&live_path.path)
                    })
            })?;

        if none_removed {
            println!("No hard links found which require removal.");
            return Ok(());
        }

        Ok(())
    }

    fn preserve_snap_links(&self) -> HttmResult<()> {
        let mut none_preserved = true;

        self.snap_map
            .link_map
            .iter()
            .try_for_each(|(_key, values)| {
                let complemented_paths: Vec<(PathBuf, &PathBuf)> = values
                    .iter()
                    .map(|snap_path| {
                        let live_path = self
                            .live_path(&snap_path.path)
                            .expect("Could obtain live path for snap path");

                        (live_path, &snap_path.path)
                    })
                    .collect();

                let mut opt_original = complemented_paths
                    .iter()
                    .map(|(live, _snap)| live)
                    .find(|path| path.exists());

                complemented_paths
                    .iter()
                    .filter(|(_live_path, snap_path)| snap_path.exists())
                    .try_for_each(|(live_path, snap_path)| {
                        none_preserved = false;

                        match opt_original {
                            Some(original) if original == live_path => {
                                RollForward::copy(snap_path, live_path)
                            }
                            Some(original) => Self::hard_link(original, live_path),
                            None => {
                                opt_original = Some(live_path);
                                RollForward::copy(snap_path, live_path)
                            }
                        }
                    })
            })?;

        if none_preserved {
            println!("No hard links found which require preservation.");
            return Ok(());
        }

        Ok(())
    }

    fn preserve_orphans(&self) -> HttmResult<()> {
        // in self but not in other
        let snap_to_live: HashSet<PathBuf> = self
            .snap_map
            .remainder
            .par_iter()
            .map(|snap_path| {
                let live_path = self
                    .live_path(&snap_path)
                    .expect("Could obtain live path for snap path");

                live_path
            })
            .collect();

        let live_diff = self.live_map.remainder.difference(&snap_to_live);
        let snap_diff = snap_to_live.difference(&self.live_map.remainder);

        // means we want to delete these
        live_diff
            .into_iter()
            .par_bridge()
            .try_for_each(|path| RollForward::remove(path))?;

        // means we want to copy these
        snap_diff
            .into_iter()
            .par_bridge()
            .try_for_each(|live_path| {
                let snap_path = RollForward::snap_path(&self.roll_forward, &live_path)
                    .expect("Could not covert to snap path.");

                RollForward::copy(&snap_path, &live_path)
            })?;

        Ok(())
    }

    fn live_path(&self, snap_path: &Path) -> Option<PathBuf> {
        snap_path
            .strip_prefix(&self.roll_forward.proximate_dataset_mount)
            .ok()
            .and_then(|path| path.strip_prefix(ZFS_SNAPSHOT_DIRECTORY).ok())
            .and_then(|path| path.strip_prefix(&self.roll_forward.snap_name).ok())
            .map(|relative_path| {
                [
                    self.roll_forward.proximate_dataset_mount.as_path(),
                    relative_path,
                ]
                .into_iter()
                .collect()
            })
    }

    fn hard_link(original: &Path, link: &Path) -> HttmResult<()> {
        if !original.exists() {
            let msg = format!(
                "Cannot link because original path does not exists: {:?}",
                original
            );
            return Err(HttmError::new(&msg).into());
        }

        if link.exists() {
            if let Ok(og_md) = original.metadata() {
                if let Ok(link_md) = link.metadata() {
                    if og_md.ino() == link_md.ino() {
                        return Ok(());
                    }
                }
            }

            remove_file(link)?
        }

        if let Err(err) = std::fs::hard_link(original, link) {
            if !link.exists() {
                eprintln!("Error: {}", err);
                let msg = format!("Could not link file {:?} to {:?}", original, link);
                return Err(HttmError::new(&msg).into());
            }
        }

        copy_attributes(original, link)?;
        is_metadata_same(original, link)?;
        eprintln!("{}: {:?} -> {:?}", Yellow.paint("Linked  "), original, link);

        Ok(())
    }

    fn rm_hard_link(link: &Path) -> HttmResult<()> {
        match std::fs::remove_file(link) {
            Ok(_) => {
                if link.exists() {
                    let msg = format!("Target link should not exist after removal {:?}", link);
                    return Err(HttmError::new(&msg).into());
                }
            }
            Err(err) => {
                if link.exists() {
                    eprintln!("Error: {}", err);
                    let msg = format!("Could not remove link {:?}", link);
                    return Err(HttmError::new(&msg).into());
                }
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Green.paint("Unlinked  "), link);

        Ok(())
    }
}
