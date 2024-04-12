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
use crate::library::file_ops::Copy;
use crate::library::file_ops::Preserve;
use crate::library::file_ops::Remove;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::{PrecautionarySnapType, SnapGuard};
use crate::library::utility::is_metadata_same;
use crate::library::utility::user_has_effective_root;
use crate::roll_forward::preserve_hard_links::PreserveHardLinks;
use crate::roll_forward::preserve_hard_links::SpawnPreserveLinks;
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};

use crate::library::iter_extensions::HttmIter;
use crate::roll_forward::diff_events::DiffEvent;
use crate::roll_forward::diff_events::DiffType;

use indicatif::ProgressBar;
use nu_ansi_term::Color::{Blue, Red};
use rayon::prelude::*;
use which::which;

use std::fs::read_dir;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command as ExecProcess, Stdio};

pub struct RollForward {
    dataset: PathBuf,
    snap: String,
    progress_bar: ProgressBar,
    pub proximate_dataset_mount: PathBuf,
}

impl RollForward {
    pub fn new(full_snap_name: &str) -> HttmResult<Self> {
        let (dataset, snap) = if let Some(res) = full_snap_name.split_once('@') {
            res
        } else {
            let msg = format!("\"{}\" is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.", &full_snap_name);
            return Err(HttmError::new(&msg).into());
        };

        let dataset = PathBuf::from(&dataset);

        let proximate_dataset_mount = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .iter()
            .find(|(_mount, md)| md.source == dataset)
            .map(|(mount, _)| mount.to_owned())
            .ok_or_else(|| HttmError::new("Could not determine proximate dataset mount"))?;

        let progress_bar: ProgressBar = indicatif::ProgressBar::new_spinner();

        Ok(Self {
            dataset,
            snap: snap.to_string(),
            progress_bar,
            proximate_dataset_mount,
        })
    }

    fn full_name(&self) -> String {
        format!("{}@{}", self.dataset.to_string_lossy(), self.snap)
    }

    pub fn exec(&self) -> HttmResult<()> {
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
        )
        .map(|_res| ())
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

        let mut process_handle = self.zfs_diff_cmd()?;

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

        self.verify()
    }

    fn verify(&self) -> HttmResult<()> {
        let snap_dataset = self.snap_dataset();

        let mut first_pass: Vec<PathBuf> = vec![snap_dataset.clone()];
        let mut second_pass = Vec::new();

        eprint!("Verifying files and symlinks: ");
        while let Some(item) = first_pass.pop() {
            let (vec_dirs, vec_files): (Vec<PathBuf>, Vec<PathBuf>) = read_dir(&item)?
                .flatten()
                .map(|dir_entry| dir_entry.path())
                .partition(|path| path.is_dir());

            // change attrs on dir when at the top of a dir tree, so not over written from above
            if vec_dirs.is_empty() {
                let live_path = self
                    .live_path(&item)
                    .ok_or_else(|| HttmError::new("Could not generate live path"))?;

                Preserve::recursive(&item, &live_path)?
            }

            first_pass.extend(vec_dirs.clone());
            second_pass.extend(vec_dirs);

            // first pass only verify non-directories
            vec_files.into_iter().try_for_each(|path| {
                self.progress_bar.tick();
                let live_path = self
                    .live_path(&path)
                    .ok_or_else(|| HttmError::new("Could not generate live path"))?;

                is_metadata_same(&path, &live_path)
            })?;
        }
        self.progress_bar.finish_and_clear();
        eprintln!("OK");

        eprint!("Verifying directories: ");
        // copy attributes for base dataset, our recursive attr copy does stops
        // before including the base dataset
        let live_dataset = self
            .live_path(&snap_dataset)
            .ok_or_else(|| HttmError::new("Could not generate live path"))?;

        Preserve::direct(&snap_dataset, &live_dataset)?;

        // 2nd pass checks dirs - why?  we don't check dirs on first pass,
        // because copying of data may have changed dir size/mtime
        second_pass.into_iter().try_for_each(|path| {
            self.progress_bar.tick();
            let live_path = self
                .live_path(&path)
                .ok_or_else(|| HttmError::new("Could not generate live path"))?;

            is_metadata_same(&path, &live_path)
        })?;
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
                [self.proximate_dataset_mount.as_path(), relative_path]
                    .into_iter()
                    .collect()
            })
    }

    fn zfs_diff_cmd(&self) -> HttmResult<Child> {
        let zfs_command = which("zfs").map_err(|_err| {
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
        })?;

        // -H: tab separated, -t: Specify time, -h: Normalize paths (don't use escape codes)
        let full_name = self.full_name();
        let process_args = vec!["diff", "-H", "-t", "-h", &full_name];

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
                    self.proximate_dataset_mount.as_path(),
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

                if snap_file_path.exists() {
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

        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    pub fn snap_dataset(&self) -> PathBuf {
        [
            self.proximate_dataset_mount.as_path(),
            Path::new(ZFS_SNAPSHOT_DIRECTORY),
            Path::new(&self.snap),
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

    pub fn remove(dst: &Path) -> HttmResult<()> {
        // overwrite
        if !dst.exists() {
            return Ok(());
        }

        match Remove::recursive_quiet(dst) {
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
