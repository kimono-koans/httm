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
use std::fs::Permissions;
use std::fs::read_dir;
use std::fs::set_permissions;
use std::io::{BufRead, Read};
use std::os::unix::fs::chown;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout};
use std::sync::Arc;

struct DirectoryLock {
    path: Box<Path>,
    uid: u32,
    gid: u32,
    permissions: Permissions,
}

impl DirectoryLock {
    fn new(proximate_dataset_mount: &Path) -> HttmResult<Self> {
        let path = proximate_dataset_mount;
        let md = path.metadata()?;

        let permissions = md.permissions();
        let uid = md.uid();
        let gid = md.gid();

        Ok(Self {
            path: path.into(),
            uid,
            gid,
            permissions,
        })
    }

    fn lock(&self) -> HttmResult<()> {
        let exclusive = Permissions::from_mode(0o600);
        let root_uid = 0;
        let root_gid = 0;

        eprintln!("Locking dataset: {:?}", self.path);

        // Mode
        {
            set_permissions(&self.path, exclusive)?
        }

        // Ownership
        {
            chown(&self.path, Some(root_uid), Some(root_gid))?
        }

        Ok(())
    }

    fn unlock(&self) -> HttmResult<()> {
        eprintln!("Unlocking dataset: {:?}", self.path);

        // Mode
        {
            set_permissions(&self.path, self.permissions.clone())?
        }

        // Ownership
        {
            chown(&self.path, Some(self.uid), Some(self.gid))?
        }

        Ok(())
    }

    fn wrap_function<F>(&self, action: F) -> HttmResult<()>
    where
        F: Fn() -> HttmResult<()>,
    {
        self.lock()?;
        let res = action();
        self.unlock()?;

        res
    }
}

pub struct RollForward {
    dataset: String,
    snap: String,
    progress_bar: ProgressBar,
    proximate_dataset_mount: Arc<Path>,
    directory_lock: DirectoryLock,
}

impl RollForward {
    pub fn new(full_snap_name: &str) -> HttmResult<Self> {
        let (dataset, snap) = if let Some(res) = full_snap_name.split_once('@') {
            res
        } else {
            let description = format!(
                "\"{}\" is not a valid data set name.  A valid ZFS snapshot name requires a '@' separating dataset name and snapshot name.",
                &full_snap_name
            );
            return HttmError::from(description).into();
        };

        let source_device = Path::new(&dataset);

        let proximate_dataset_mount = Self::proximate_dataset_from_source(source_device)?;

        let progress_bar: ProgressBar = indicatif::ProgressBar::new_spinner();

        let directory_lock = DirectoryLock::new(&proximate_dataset_mount)?;

        Ok(Self {
            dataset: dataset.to_string(),
            snap: snap.to_string(),
            progress_bar,
            proximate_dataset_mount,
            directory_lock,
        })
    }

    fn proximate_dataset_from_source(source_device: &Path) -> HttmResult<Arc<Path>> {
        GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .iter()
            .find(|(_mount, md)| md.source.as_ref() == source_device)
            .map(|(mount, _)| mount.clone())
            .ok_or_else(|| HttmError::new("Could not determine proximate dataset mount").into())
    }

    pub fn proximate_dataset_mount(&self) -> &Path {
        self.proximate_dataset_mount.as_ref()
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

        match self.directory_lock.wrap_function(|| self.roll_forward()) {
            Ok(_) => {
                println!("httm roll forward completed successfully.");
            }
            Err(err) => {
                let description = format!(
                    "httm roll forward failed for the following reason: {}.\n\
                Attempting roll back to precautionary pre-execution snapshot.",
                    err
                );
                eprintln!("{}", description);

                snap_guard
                    .rollback()
                    .map(|_| println!("Rollback succeeded."))?;

                std::process::exit(1)
            }
        };

        SnapGuard::new(
            &self.dataset,
            PrecautionarySnapType::PostRollForward(self.snap.clone()),
        )?;

        Ok(())
    }

    fn roll_forward(&self) -> HttmResult<()> {
        let spawn_res = SpawnPreserveLinks::new(self);

        let run_zfs = RunZFSCommand::new()?;

        let mut process_handle = run_zfs.diff(&self)?;

        let opt_stderr = process_handle.stderr.take();
        let mut opt_stdout = process_handle.stdout.take();

        let stream = Self::ingest(&mut opt_stdout)?;

        let mut stream_peekable = stream.peekable();

        if stream_peekable.peek().is_none() {
            let err_string = Self::zfs_diff_std_err(opt_stderr)?;

            if err_string.is_empty() {
                return HttmError::new("'zfs diff' reported no changes to dataset").into();
            }

            return HttmError::from(err_string).into();
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
                eprintln!(
                    "NOTICE: 'zfs diff' reported an error.  At this point of execution, these are usually inconsequential: {}",
                    buf.trim()
                );
            }
        }

        if !parse_errors.is_empty() {
            let description: String = parse_errors.into_iter().map(|e| e.to_string()).collect();
            return HttmError::from(description).into();
        }

        let exclusions = PreserveHardLinks::try_from(spawn_res)?.exec()?;

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

        let mut directory_list: Vec<PathBuf> = Vec::new();
        let mut file_list: Vec<PathBuf> = Vec::new();
        let mut queue: Vec<PathBuf> = vec![snap_dataset.clone()];

        eprint!("Building file and directory list: ");
        while let Some(item) = queue.pop() {
            let (mut vec_dirs, mut vec_files): (Vec<PathBuf>, Vec<PathBuf>) = read_dir(&item)?
                .flatten()
                .map(|dir_entry| dir_entry.path())
                .partition(|path| path.is_dir());

            queue.extend_from_slice(&vec_dirs);
            directory_list.append(&mut vec_dirs);
            file_list.append(&mut vec_files);
        }
        eprintln!("OK");

        // first pass only verify non-directories
        eprint!("Verifying files and symlinks: ");

        self.verify_from_list(file_list)?;

        self.progress_bar.finish_and_clear();
        eprintln!("OK");

        eprint!("Verifying directories: ");
        // 2nd pass checks dirs - why?  we don't check dirs on first pass,
        // because copying of data may have changed dir size/mtime
        self.verify_from_list(directory_list)?;

        self.progress_bar.finish_and_clear();
        eprintln!("OK");

        // copy attributes for base dataset, our recursive attr copy stops
        // before including the base dataset
        if let Some(live_dataset) = self.live_path(&snap_dataset) {
            let _ = Preserve::direct(&snap_dataset, &live_dataset);
        }

        Ok(())
    }

    fn verify_from_list(&self, mut list: Vec<PathBuf>) -> HttmResult<()> {
        list.sort_unstable();
        // reverse because we want to work from the bottom up
        list.reverse();

        list.iter()
            .filter_map(|snap_path| {
                self.live_path(&snap_path)
                    .map(|live_path| (snap_path, live_path))
            })
            .filter_map(|(snap_path, live_path)| {
                self.progress_bar.tick();

                match is_metadata_same(&snap_path, &&live_path) {
                    Ok(_) => None,
                    Err(_) => Some((snap_path, live_path)),
                }
            })
            .try_for_each(|(snap_path, live_path)| {
                // zfs diff sometimes doesn't pick up some rename events
                // here we cleanup
                eprintln!("DEBUG: Cleanup required {:?} -> {:?}", snap_path, live_path);
                Self::overwrite_or_remove(&snap_path, &live_path)?;

                is_metadata_same(&snap_path, &&live_path)
            })
    }

    fn zfs_diff_std_err(opt_stderr: Option<ChildStderr>) -> HttmResult<String> {
        let mut buf = String::new();

        if let Some(mut stderr) = opt_stderr {
            stderr.read_to_string(&mut buf)?;
        }

        Ok(buf)
    }

    pub fn live_path(&self, snap_path: &Path) -> Option<PathBuf> {
        snap_path
            .strip_prefix(&self.proximate_dataset_mount)
            .ok()
            .and_then(|path| path.strip_prefix(ZFS_SNAPSHOT_DIRECTORY).ok())
            .and_then(|path| path.strip_prefix(&self.snap).ok())
            .map(|relative_path| {
                let mut live_path = self.proximate_dataset_mount.to_path_buf();
                live_path.push(relative_path);

                live_path
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
            None => HttmError::new("'zfs diff' reported no changes to dataset").into(),
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
            _ => HttmError::new("Could not parse diff event").into(),
        }
    }

    pub fn snap_path(&self, path: &Path) -> Option<PathBuf> {
        PathData::from(path)
            .relative_path(&self.proximate_dataset_mount)
            .ok()
            .map(|relative_path| {
                let mut snap_file_path: PathBuf = self.proximate_dataset_mount.to_path_buf();

                snap_file_path.push(ZFS_SNAPSHOT_DIRECTORY);
                snap_file_path.push(&self.snap);
                snap_file_path.push(relative_path);

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
            let description = format!(
                "Could not overwrite {:?} with snapshot file version {:?}",
                dst, src
            );
            return HttmError::from(description).into();
        }

        Preserve::direct(src, dst)?;

        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    pub fn snap_dataset(&self) -> PathBuf {
        let mut path = self.proximate_dataset_mount.to_path_buf();

        path.push(ZFS_SNAPSHOT_DIRECTORY);
        path.push(&self.snap);

        path
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
                    let description = format!("File should not exist after deletion {:?}", dst);
                    return HttmError::from(description).into();
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                let description = format!("Could not delete file {:?}", dst);
                return HttmError::from(description).into();
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Red.paint("Removed  "), dst);

        Ok(())
    }
}
