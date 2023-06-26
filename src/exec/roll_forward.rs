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
use std::sync::atomic::AtomicBool;
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
use crate::library::utility::{copy_direct, remove_recursive};
use crate::library::utility::{is_metadata_same, user_has_effective_root};
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};

#[derive(Debug, Clone)]
struct DiffEvent {
    path_buf: PathBuf,
    diff_type: DiffType,
    time: DiffTime,
}

impl DiffEvent {
    fn new(path_string: &str, diff_type: DiffType, time_str: &str) -> HttmResult<Self> {
        let path_buf = PathBuf::from(&path_string);

        Ok(Self {
            path_buf,
            diff_type,
            time: DiffTime::new(time_str)?,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct DiffTime {
    secs: u64,
    nanos: u64,
}

impl DiffTime {
    fn new(time_str: &str) -> HttmResult<Self> {
        let (secs, nanos) = time_str
            .split_once('.')
            .ok_or_else(|| HttmError::new("Could not split time string."))?;

        let time = DiffTime {
            secs: secs.parse::<u64>()?,
            nanos: nanos.parse::<u64>()?,
        };

        Ok(time)
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

    fn roll_forward(&self) -> HttmResult<()> {
        let (snap_handle, live_handle) = self.spawn_preserve_links();

        let mut process_handle = self.zfs_diff_cmd()?;

        let opt_stderr = process_handle.stderr.take();
        let mut opt_stdout = process_handle.stdout.take();

        let stream = Self::ingest(&mut opt_stdout)?;

        let mut stream_peekable = stream.peekable();

        if stream_peekable.peek().is_none() {
            return Err(HttmError::new("'zfs diff' reported no changes to dataset").into());
        }

        // zfs-diff can return multiple file actions for a single inode, here we dedup
        eprintln!("Building a map of ZFS filesystem events since the specified snapshot:");
        let mut parse_errors = vec![];
        let group_map = stream_peekable
            .map(|event| {
                self.roll_config.progress_bar.tick();
                event
            })
            .filter_map(|res| res.map_err(|e| parse_errors.push(e)).ok())
            .into_group_map_by(|event| event.path_buf.clone());

        if let Some(mut stderr) = opt_stderr {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf)?;

            if !buf.is_empty() {
                let msg = format!("'zfs diff' command reported an error: {}", buf);
                return Err(HttmError::new(&msg).into());
            }
        }

        if !parse_errors.is_empty() {
            let msg: String = parse_errors.into_iter().map(|e| e.to_string()).collect();
            return Err(HttmError::new(&msg).into());
        }

        // need to wait for these to finish before executing any diff_action
        let snap_map = snap_handle
            .join()
            .map_err(|_err| HttmError::new("Thread panicked!"))??;

        let live_map = live_handle
            .join()
            .map_err(|_err| HttmError::new("Thread panicked!"))??;

        let preserve_hard_links = PreserveHardLinks::new(&live_map, &snap_map, self.to_owned())?;

        eprintln!("Preserving possibly previously linked orphans:");
        let snaps_to_live = preserve_hard_links.snaps_to_live()?;
        let mut exclusions = preserve_hard_links.preserve_orphans(&snaps_to_live)?;
        Self::exclusions(&mut exclusions, &live_map, &snap_map);

        eprintln!("Removing unnecessary links on the live dataset:");
        preserve_hard_links.preserve_live_links()?;

        eprintln!("Preserving necessary links from the snapshot dataset:");
        preserve_hard_links.preserve_snap_links()?;

        // into iter and reverse because we want to go largest first
        eprintln!("Reversing 'zfs diff' actions:");
        group_map
            .par_iter()
            .filter(|(key, _values)| !exclusions.contains(key))
            .flat_map(|(_key, values)| values.iter().max_by_key(|event| event.time))
            .try_for_each(|event| {
                let snap_file_path = self.snap_path(&event.path_buf).ok_or_else(|| {
                    HttmError::new("Could not obtain snap file path for live version.")
                })?;

                self.diff_action(event, &snap_file_path)
            })?;

        eprintln!("Verifying path names and metadata match snapshot source:");
        self.verify()
    }

    fn exclusions<'a>(
        potential_orphans: &mut HashSet<&'a PathBuf>,
        live_map: &'a HardLinkMap,
        snap_map: &'a HardLinkMap,
    ) {
        potential_orphans.extend(
            live_map
                .link_map
                .values()
                .flatten()
                .map(|entry| &entry.path),
        );

        potential_orphans.extend(
            snap_map
                .link_map
                .values()
                .flatten()
                .map(|entry| &entry.path),
        );
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
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(process_handle)
    }

    fn ingest(
        output: &mut Option<ChildStdout>,
    ) -> HttmResult<impl Iterator<Item = HttmResult<DiffEvent>> + '_> {
        const IN_BUFFER_SIZE: usize = 65_536;

        match output {
            Some(output) => {
                let stdout_buffer = std::io::BufReader::with_capacity(IN_BUFFER_SIZE, output);

                let ret = stdout_buffer.lines().map(|res| {
                    res.map_err(|e| e.into())
                        .and_then(|line| Self::ingest_by_line(&line))
                });

                Ok(ret)
            }
            None => Err(HttmError::new("'zfs diff' reported no changes to dataset").into()),
        }
    }

    fn ingest_by_line(line: &str) -> HttmResult<DiffEvent> {
        let split_line: Vec<&str> = line.split('\t').collect();

        let time_str = split_line
            .first()
            .ok_or_else(|| HttmError::new("Could not obtain a timestamp for diff event."))?;

        let path = split_line
            .get(2)
            .ok_or_else(|| HttmError::new("Could not obtain a path for diff event."))?;

        match split_line.get(1) {
            Some(&"-") => DiffEvent::new(path, DiffType::Removed, time_str),
            Some(&"+") => DiffEvent::new(path, DiffType::Created, time_str),
            Some(&"M") => DiffEvent::new(path, DiffType::Modified, time_str),
            Some(&"R") => {
                let new_file_name = split_line.get(3).ok_or_else(|| {
                    HttmError::new("Could not obtain a new file name for diff event.")
                })?;

                DiffEvent::new(
                    path,
                    DiffType::Renamed(PathBuf::from(new_file_name)),
                    time_str,
                )
            }
            Some(_) => Err(HttmError::new("Could not obtain a diff type for diff event.").into()),
            _ => Err(HttmError::new("Could not parse diff event").into()),
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
            DiffType::Removed | DiffType::Modified => Self::copy(snap_file_path, &event.path_buf),
            DiffType::Created => Self::overwrite_or_remove(snap_file_path, &event.path_buf),
            DiffType::Renamed(new_file_name) => {
                Self::overwrite_or_remove(snap_file_path, new_file_name)
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

    fn verify(&self) -> HttmResult<()> {
        let snap_dataset = self.snap_dataset();

        let constructed = BasicDirEntryInfo {
            path: snap_dataset,
            file_type: None,
        };

        let mut queue: Vec<BasicDirEntryInfo> = vec![constructed];

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
                .map(|snap_entry| {
                    self.roll_config.progress_bar.tick();
                    snap_entry
                })
                .try_for_each(|snap_entry| {
                    let live_path = self
                        .live_path(&snap_entry.path)
                        .ok_or_else(|| HttmError::new("Could not obtain live path"))?;

                    is_metadata_same(snap_entry.path, live_path)
                })?;
        }

        Ok(())
    }

    fn live_path(&self, snap_path: &Path) -> Option<PathBuf> {
        snap_path
            .strip_prefix(&self.proximate_dataset_mount)
            .ok()
            .and_then(|path| path.strip_prefix(ZFS_SNAPSHOT_DIRECTORY).ok())
            .and_then(|path| path.strip_prefix(&self.snap_name).ok())
            .map(|relative_path| {
                [self.proximate_dataset_mount.as_path(), relative_path]
                    .into_iter()
                    .collect()
            })
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
                        return ft.is_file();
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
        ) = tmp.into_iter().partition(|(_ino, values)| values.len() > 1);

        let remainder = remain_tmp
            .into_values()
            .flatten()
            .map(|entry| entry.path)
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
        let none_removed = AtomicBool::new(true);

        self.live_map
            .link_map
            .iter()
            .try_for_each(|(_key, values)| {
                values.iter().try_for_each(|live_path| {
                    let snap_path: HttmResult<PathBuf> =
                        self.roll_forward.snap_path(&live_path.path).ok_or_else(|| {
                            HttmError::new("Could obtain live path for snap path").into()
                        });

                    if !snap_path?.exists() {
                        none_removed.store(false, std::sync::atomic::Ordering::Relaxed);
                        return Self::rm_hard_link(&live_path.path);
                    }

                    Ok(())
                })
            })?;

        if none_removed.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("No hard links found which require removal.");
            return Ok(());
        }

        Ok(())
    }

    fn preserve_snap_links(&self) -> HttmResult<()> {
        let none_preserved = AtomicBool::new(true);

        self.snap_map
            .link_map
            .par_iter()
            .try_for_each(|(_key, values)| {
                let complemented_paths: Vec<(PathBuf, &PathBuf)> = values
                    .iter()
                    .map(|snap_path| {
                        let live_path = self.live_path(&snap_path.path).ok_or_else(|| {
                            HttmError::new("Could obtain live path for snap path").into()
                        });

                        live_path.map(|live| (live, &snap_path.path))
                    })
                    .collect::<HttmResult<Vec<(PathBuf, &PathBuf)>>>()?;

                let mut opt_original = complemented_paths.iter().find_map(|(live, _snap)| {
                    if live.exists() {
                        Some(live)
                    } else {
                        None
                    }
                });

                complemented_paths
                    .iter()
                    .filter(|(_live_path, snap_path)| snap_path.exists())
                    .try_for_each(|(live_path, snap_path)| {
                        none_preserved.store(false, std::sync::atomic::Ordering::Relaxed);

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

        if none_preserved.load(std::sync::atomic::Ordering::Relaxed) {
            println!("No hard links found which require preservation.");
            return Ok(());
        }

        Ok(())
    }

    fn snaps_to_live(&self) -> HttmResult<HashSet<PathBuf>> {
        // in self but not in other
        self.snap_map
            .remainder
            .par_iter()
            .map(|snap_path| {
                self.live_path(snap_path)
                    .ok_or_else(|| HttmError::new("Could obtain live path for snap path").into())
            })
            .collect::<HttmResult<HashSet<PathBuf>>>()
    }

    fn preserve_orphans(
        &'a self,
        snaps_to_live: &'a HashSet<PathBuf>,
    ) -> HttmResult<HashSet<&'a PathBuf>> {
        let live_diff = self.live_map.remainder.difference(snaps_to_live);
        let snap_diff = snaps_to_live.difference(&self.live_map.remainder);

        // means we want to delete these
        live_diff
            .clone()
            .par_bridge()
            .try_for_each(|path| RollForward::remove(path))?;

        // means we want to copy these
        snap_diff.clone().par_bridge().try_for_each(|live_path| {
            let snap_path: HttmResult<PathBuf> =
                RollForward::snap_path(self.roll_forward, live_path)
                    .ok_or_else(|| HttmError::new("Could obtain live path for snap path").into());

            RollForward::copy(&snap_path?, live_path)
        })?;

        let combined: HashSet<&PathBuf> = live_diff.chain(snap_diff).collect();

        Ok(combined)
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
