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

use crate::data::paths::{PathData, PathDeconstruction};
use crate::library::diff_copy::DiffCopy;
use crate::library::file_ops::{Copy, Preserve, Remove};
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{is_metadata_same, user_has_effective_root};
use crate::roll_forward::diff_events::{DiffEvent, DiffType};
use crate::roll_forward::preserve_hard_links::{PreserveHardLinks, SpawnPreserveLinks};
use crate::zfs::run_command::RunZFSCommand;
use crate::zfs::snap_guard::{PrecautionarySnapType, SnapGuard};
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};
use indicatif::ProgressBar;
use nu_ansi_term::Color::{Blue, Red};
use rayon::prelude::*;
use std::fs::read_dir;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout};
use std::sync::Arc;

pub struct RollForward {
    dataset: String,
    snap: String,
    progress_bar: ProgressBar,
    pub proximate_dataset_mount: Arc<Path>,
}

impl RollForward {
    pub fn new(full_snap_name: &str) -> HttmResult<Self> {
        let (dataset, snap) = if let Some(res) = full_snap_name.split_once('@') {
            res
        } else {
            let msg = format!("\"{}\" is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.", &full_snap_name);
            return Err(HttmError::new(&msg).into());
        };

        let dataset_path = Path::new(&dataset);

        let proximate_dataset_mount = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .iter()
            .find(|(_mount, md)| md.source.as_ref() == dataset_path)
            .map(|(mount, _)| mount.clone())
            .ok_or_else(|| HttmError::new("Could not determine proximate dataset mount"))?;

        let progress_bar: ProgressBar = indicatif::ProgressBar::new_spinner();

        Ok(Self {
            dataset: dataset.to_string(),
            snap: snap.to_string(),
            progress_bar,
            proximate_dataset_mount,
        })
    }

    pub fn full_name(&self) -> String {
        format!("{}@{}", self.dataset, self.snap)
    }

    pub fn exec(&self) -> HttmResult<()> {
        // ZFS allow is not sufficient so a ZFSAllowPriv guard isn't here either
        // we need root, so we do a raw SnapGuard after checking that we have root
        user_has_effective_root("Roll forward to a snapshot.")?;

        let snap_guard: SnapGuard =
            SnapGuard::new(&self.dataset, PrecautionarySnapType::PreRollForward)?;

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
            &self.dataset,
            PrecautionarySnapType::PostRollForward(self.snap.to_owned()),
        )?;

        Ok(())
    }

    fn zfs_diff_std_err(opt_stderr: Option<ChildStderr>) -> HttmResult<String> {
        let mut buf = String::new();

        if let Some(mut stderr) = opt_stderr {
            stderr.read_to_string(&mut buf)?;
        }

        Ok(buf)
    }

    fn roll_forward(&self) -> HttmResult<()> {
        let spawn_res = SpawnPreserveLinks::new(self);

        let (snap_handle, live_handle) = (spawn_res.snap_handle, spawn_res.live_handle);

        let run_zfs = RunZFSCommand::new()?;

        let mut process_handle = run_zfs.diff(&self)?;

        let opt_stderr = process_handle.stderr.take();
        let mut opt_stdout = process_handle.stdout.take();

        let stream = Self::ingest(&mut opt_stdout)?;

        let mut stream_peekable = stream.peekable();

        if stream_peekable.peek().is_none() {
            let msg = Self::zfs_diff_std_err(opt_stderr)?;

            if msg.is_empty() {
                return Err(HttmError::new("'zfs diff' reported no changes to dataset").into());
            }

            return Err(HttmError::new(&msg).into());
        }

        // zfs-diff can return multiple file actions for a single inode, here we dedup
        eprintln!("Building a map of ZFS filesystem events since the specified snapshot.");
        let mut parse_errors = vec![];
        let group_map = stream_peekable
            .map(|event| {
                self.progress_bar.tick();
                event
            })
            .filter_map(|res| res.map_err(|e| parse_errors.push(e)).ok())
            .into_group_map_by(|event| event.path_buf.clone());
        self.progress_bar.finish_and_clear();

        // These errors usually don't matter, if we make it this far.  Most are of the form:
        // "Unable to determine path or stats for object 99694 in ...: File exists"
        // Here, we print only as NOTICE
        if let Ok(buf) = Self::zfs_diff_std_err(opt_stderr) {
            if !buf.is_empty() {
                eprintln!("NOTICE: 'zfs diff' reported an error.  At this point of execution, these are usually inconsequential: {}", buf.trim());
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
        let exclusions = preserve_hard_links.exec()?;

        // into iter and reverse because we want to go largest first
        eprintln!("Reversing 'zfs diff' actions.");
        group_map
            .par_iter()
            .filter(|(key, _values)| !exclusions.contains(key.as_path()))
            .flat_map(|(_key, values)| values.iter().max_by_key(|event| event.time))
            .try_for_each(|event| match &event.diff_type {
                DiffType::Renamed(new_file) if exclusions.contains(new_file) => Ok(()),
                _ => self.diff_action(event),
            })?;

        self.cleanup_and_verify()
    }

    fn cleanup_and_verify(&self) -> HttmResult<()> {
        let snap_dataset = self.snap_dataset();

        let mut directory_list: Vec<PathBuf> = vec![snap_dataset.clone()];
        let mut file_list: Vec<PathBuf> = Vec::new();

        eprint!("Building file and directory list: ");
        while let Some(item) = directory_list.pop() {
            let (mut vec_dirs, mut vec_files): (Vec<PathBuf>, Vec<PathBuf>) = read_dir(&item)?
                .flatten()
                .map(|dir_entry| dir_entry.path())
                .partition(|path| path.is_dir());

            directory_list.append(&mut vec_dirs);
            file_list.append(&mut vec_files);
        }
        eprintln!("OK");

        eprint!("Verifying files and symlinks: ");
        // first pass only verify non-directories
        file_list.sort_by_key(|path| path.components().count());

        file_list.reverse();

        file_list
            .into_iter()
            .filter_map(|snap_path| {
                self.live_path(&snap_path)
                    .map(|live_path| (snap_path, live_path))
            })
            .try_for_each(|(snap_path, live_path)| {
                self.progress_bar.tick();

                is_metadata_same(&snap_path, &live_path)
            })?;

        self.progress_bar.finish_and_clear();
        eprintln!("OK");

        eprint!("Verifying directories: ");
        // 2nd pass checks dirs - why?  we don't check dirs on first pass,
        // because copying of data may have changed dir size/mtime
        directory_list.sort_by_key(|path| path.components().count());

        directory_list.reverse();

        directory_list
            .into_iter()
            .filter_map(|snap_path| {
                self.live_path(&snap_path)
                    .map(|live_path| (snap_path, live_path))
            })
            .try_for_each(|(snap_path, live_path)| {
                self.progress_bar.tick();

                Preserve::direct(&snap_path, &live_path)?;

                is_metadata_same(&snap_path, &live_path)
            })?;

        // copy attributes for base dataset, our recursive attr copy does stops
        // before including the base dataset
        let live_dataset = self
            .live_path(&snap_dataset)
            .ok_or_else(|| HttmError::new("Could not generate live path"))?;

        let _ = Preserve::direct(&snap_dataset, &live_dataset);

        self.progress_bar.finish_and_clear();

        eprintln!("OK");

        Ok(())
    }

    pub fn live_path(&self, snap_path: &Path) -> Option<PathBuf> {
        snap_path
            .strip_prefix(&self.proximate_dataset_mount)
            .ok()
            .and_then(|path| path.strip_prefix(ZFS_SNAPSHOT_DIRECTORY).ok())
            .and_then(|path| path.strip_prefix(&self.snap).ok())
            .map(|relative_path| {
                [self.proximate_dataset_mount.as_ref(), relative_path]
                    .into_iter()
                    .collect()
            })
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

        let diff_type = split_line.get(1);

        let path = split_line
            .get(2)
            .ok_or_else(|| HttmError::new("Could not obtain a path for diff event."))?;

        match diff_type {
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
            _ => Err(HttmError::new("Could not parse diff event").into()),
        }
    }

    pub fn snap_path(&self, path: &Path) -> Option<PathBuf> {
        PathData::from(path)
            .relative_path(&self.proximate_dataset_mount)
            .ok()
            .map(|relative_path| {
                let snap_file_path: PathBuf = [
                    self.proximate_dataset_mount.as_ref(),
                    Path::new(ZFS_SNAPSHOT_DIRECTORY),
                    Path::new(&self.snap),
                    relative_path,
                ]
                .iter()
                .collect();

                snap_file_path
            })
    }

    fn diff_action(&self, event: &DiffEvent) -> HttmResult<()> {
        let snap_file_path = self
            .snap_path(&event.path_buf)
            .ok_or_else(|| HttmError::new("Could not obtain snap file path for live version."))?;

        // zfs-diff can return multiple file actions for a single inode
        // since we exclude older file actions, if rename or created is the last action,
        // we should make sure it has the latest data, so a simple rename is not enough
        // this is internal to the fn Self::remove()
        match &event.diff_type {
            DiffType::Removed | DiffType::Modified => Self::copy(&snap_file_path, &event.path_buf),
            DiffType::Created => Self::overwrite_or_remove(&snap_file_path, &event.path_buf),
            DiffType::Renamed(new_file_name) => {
                let snap_new_file_name = self.snap_path(new_file_name).ok_or_else(|| {
                    HttmError::new("Could not obtain snap file path for live version.")
                })?;

                Self::overwrite_or_remove(&snap_new_file_name, new_file_name)?;

                if snap_file_path.try_exists()? {
                    Self::copy(&snap_file_path, &event.path_buf)?
                }

                Ok(())
            }
        }
    }

    pub fn copy(src: &Path, dst: &Path) -> HttmResult<()> {
        if let Err(err) = Copy::direct_quiet(src, dst, true) {
            eprintln!("Error: {}", err);
            let msg = format!(
                "Could not overwrite {:?} with snapshot file version {:?}",
                dst, src
            );
            return Err(HttmError::new(&msg).into());
        }

        Preserve::direct(src, dst)?;

        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    pub fn snap_dataset(&self) -> PathBuf {
        [
            self.proximate_dataset_mount.as_ref(),
            Path::new(ZFS_SNAPSHOT_DIRECTORY),
            Path::new(&self.snap),
        ]
        .iter()
        .collect()
    }

    fn overwrite_or_remove(src: &Path, dst: &Path) -> HttmResult<()> {
        // overwrite
        if src.try_exists()? {
            return Self::copy(src, dst);
        }

        // or remove
        Self::remove(dst)
    }

    pub fn remove(dst: &Path) -> HttmResult<()> {
        // overwrite
        if !dst.try_exists()? {
            return Ok(());
        }

        match Remove::recursive_quiet(dst) {
            Ok(_) => {
                if dst.try_exists()? {
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
