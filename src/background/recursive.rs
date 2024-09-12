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

use crate::background::deleted::DeletedSearch;
use crate::config::generate::{DeletedMode, ExecMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::display::wrapper::DisplayWrapper;
use crate::filesystem::mounts::{IsFilterDir, MaxLen};
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{print_output_buf, HttmIsDir};
use crate::{VersionsMap, BTRFS_SNAPPER_HIDDEN_DIRECTORY, GLOBAL_CONFIG, ZFS_HIDDEN_DIRECTORY};
use rayon::{Scope, ThreadPool};
use skim::prelude::*;
use std::fs::read_dir;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

static OPT_REQUESTED_DIR_DEV: LazyLock<u64> = LazyLock::new(|| {
    GLOBAL_CONFIG
        .opt_requested_dir
        .as_ref()
        .expect("opt_requested_dir should be Some value at this point in execution")
        .symlink_metadata()
        .expect("Cannot read metadata for directory requested for search.")
        .dev()
});

static FILTER_DIRS_MAX_LEN: LazyLock<usize> =
    LazyLock::new(|| GLOBAL_CONFIG.dataset_collection.filter_dirs.max_len());

#[derive(Clone, Copy)]
pub enum PathProvenance {
    FromLiveDataset,
    IsPhantom,
}

pub struct RecursiveSearch;

impl RecursiveSearch {
    pub fn exec(
        requested_dir: &Path,
        skim_tx: SkimItemSender,
        hangup: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    ) {
        fn run_loop(
            requested_dir: &Path,
            skim_tx: SkimItemSender,
            opt_deleted_scope: Option<&Scope>,
            hangup: Arc<AtomicBool>,
            started: Arc<AtomicBool>,
        ) {
            // this runs the main loop for live file searches, see the referenced struct below
            // we are in our own detached system thread, so print error and exit if error trickles up
            RecursiveMainLoop::exec(requested_dir, opt_deleted_scope, &skim_tx, hangup, started)
                .unwrap_or_else(|error| {
                    eprintln!("Error: {error}");
                    std::process::exit(1)
                });
        }

        if GLOBAL_CONFIG.opt_deleted_mode.is_some() {
            // thread pool allows deleted to have its own scope, which means
            // all threads must complete before the scope exits.  this is important
            // for display recursive searches as the live enumeration will end before
            // all deleted threads have completed
            let pool: ThreadPool = rayon::ThreadPoolBuilder::new()
                .build()
                .expect("Could not initialize rayon threadpool for recursive deleted search");

            pool.in_place_scope(|deleted_scope| {
                run_loop(
                    requested_dir,
                    skim_tx,
                    Some(deleted_scope),
                    hangup.clone(),
                    started,
                )
            })
        } else {
            run_loop(requested_dir, skim_tx, None, hangup, started)
        }
    }
}

// this is the main loop to recurse all files
pub struct RecursiveMainLoop;

impl RecursiveMainLoop {
    fn exec(
        requested_dir: &Path,
        opt_deleted_scope: Option<&Scope>,
        skim_tx: &SkimItemSender,
        hangup: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    ) -> HttmResult<()> {
        // the user may specify a dir for browsing,
        // but wants to restore that directory,
        // so here we add the directory and its parent as a selection item
        let dot_as_entry = BasicDirEntryInfo::new(
            requested_dir.to_path_buf(),
            Some(requested_dir.metadata()?.file_type()),
        );
        let mut initial_vec_dirs = vec![dot_as_entry];

        if let Some(parent) = requested_dir.parent() {
            let double_dot_as_entry =
                BasicDirEntryInfo::new(parent.to_path_buf(), Some(parent.metadata()?.file_type()));

            initial_vec_dirs.push(double_dot_as_entry)
        }

        SharedRecursive::combine_and_send_entries(
            vec![],
            &initial_vec_dirs,
            PathProvenance::FromLiveDataset,
            requested_dir,
            skim_tx,
        )?;

        // runs once for non-recursive but also "primes the pump"
        // for recursive to have items available, also only place an
        // error can stop execution
        let mut queue: Vec<BasicDirEntryInfo> =
            Self::enter_directory(requested_dir, opt_deleted_scope, skim_tx, &hangup)?;

        started.store(true, Ordering::SeqCst);

        if GLOBAL_CONFIG.opt_recursive {
            // condition kills iter when user has made a selection
            // pop_back makes this a LIFO queue which is supposedly better for caches
            while let Some(item) = queue.pop() {
                // check -- should deleted threads keep working?
                // exit/error on disconnected channel, which closes
                // at end of browse scope
                if hangup.load(Ordering::Relaxed) {
                    break;
                }

                // no errors will be propagated in recursive mode
                // far too likely to run into a dir we don't have permissions to view
                if let Ok(mut items) =
                    Self::enter_directory(&item.path(), opt_deleted_scope, skim_tx, &hangup)
                {
                    queue.append(&mut items)
                }
            }
        }

        Ok(())
    }

    fn enter_directory(
        requested_dir: &Path,
        opt_deleted_scope: Option<&Scope>,
        skim_tx: &SkimItemSender,
        hangup: &Arc<AtomicBool>,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            SharedRecursive::entries_partitioned(requested_dir)?;

        SharedRecursive::combine_and_send_entries(
            vec_files,
            &vec_dirs,
            PathProvenance::FromLiveDataset,
            requested_dir,
            skim_tx,
        )?;

        if let Some(deleted_scope) = opt_deleted_scope {
            DeletedSearch::spawn(requested_dir, deleted_scope, skim_tx, hangup);
        }

        Ok(vec_dirs)
    }
}

pub struct SharedRecursive;

impl SharedRecursive {
    pub fn combine_and_send_entries(
        vec_files: Vec<BasicDirEntryInfo>,
        vec_dirs: &[BasicDirEntryInfo],
        is_phantom: PathProvenance,
        requested_dir: &Path,
        skim_tx: &SkimItemSender,
    ) -> HttmResult<()> {
        let mut combined = vec_files;
        combined.extend_from_slice(vec_dirs);

        let entries = match is_phantom {
            PathProvenance::FromLiveDataset => {
                // live - not phantom
                match GLOBAL_CONFIG.opt_deleted_mode {
                    Some(DeletedMode::Only) => return Ok(()),
                    Some(DeletedMode::DepthOfOne | DeletedMode::All) | None => {
                        // never show live files is display recursive/deleted only file mode
                        if matches!(
                            GLOBAL_CONFIG.exec_mode,
                            ExecMode::NonInteractiveRecursive(_)
                        ) {
                            return Ok(());
                        }
                        combined
                    }
                }
            }
            PathProvenance::IsPhantom => {
                // deleted - phantom
                Self::pseudo_live_versions(combined, requested_dir)
            }
        };

        Self::display_or_transmit(entries, is_phantom, skim_tx)
    }

    pub fn entries_partitioned(
        requested_dir: &Path,
    ) -> HttmResult<(Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>)> {
        // separates entries into dirs and files
        let (vec_dirs, vec_files) = read_dir(requested_dir)?
            .flatten()
            // checking file_type on dir entries is always preferable
            // as it is much faster than a metadata call on the path
            .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry))
            .filter(|entry| {
                if GLOBAL_CONFIG.opt_no_filter {
                    return true;
                }

                if GLOBAL_CONFIG.opt_no_hidden
                    && entry.filename().to_string_lossy().starts_with('.')
                {
                    return false;
                }

                if GLOBAL_CONFIG.opt_one_filesystem {
                    match entry.path().metadata() {
                        Ok(path_md) if *OPT_REQUESTED_DIR_DEV == path_md.dev() => {}
                        _ => {
                            // if we can't read the metadata for a path,
                            // we probably shouldn't show it either
                            return false;
                        }
                    }
                }

                if let Ok(file_type) = entry.filetype() {
                    if file_type.is_dir() {
                        return !Self::exclude_path(entry);
                    }
                }

                true
            })
            .partition(Self::is_entry_dir);

        Ok((vec_dirs, vec_files))
    }

    pub fn is_entry_dir(entry: &BasicDirEntryInfo) -> bool {
        // must do is_dir() look up on DirEntry file_type() as look up on Path will traverse links!
        if GLOBAL_CONFIG.opt_no_traverse {
            if let Ok(file_type) = entry.filetype() {
                return file_type.is_dir();
            }
        }

        entry.httm_is_dir()
    }

    fn exclude_path(entry: &BasicDirEntryInfo) -> bool {
        // FYI path is always a relative path, but no need to canonicalize as
        // partial eq for paths is comparison of components iter
        let path = entry.path();

        // never check the hidden snapshot directory for live files (duh)
        // didn't think this was possible until I saw a SMB share return
        // a .zfs dir entry
        if path.ends_with(ZFS_HIDDEN_DIRECTORY) || path.ends_with(BTRFS_SNAPPER_HIDDEN_DIRECTORY) {
            return true;
        }

        // is a common btrfs snapshot dir?
        if let Some(common_snap_dir) = &GLOBAL_CONFIG.dataset_collection.opt_common_snap_dir {
            if path == common_snap_dir.as_ref() {
                return true;
            }
        }

        // check whether user requested this dir specifically, then we will show
        if let Some(user_requested_dir) = GLOBAL_CONFIG.opt_requested_dir.as_ref() {
            if user_requested_dir.as_path() == path {
                return false;
            }
        }

        // finally : is a non-supported dataset?
        // bailout easily if path is larger than max_filter_dir len
        if path.components().count() > *FILTER_DIRS_MAX_LEN {
            return false;
        }

        path.is_filter_dir()
    }

    // this function creates dummy "live versions" values to match deleted files
    // which have been found on snapshots, we return to the user "the path that
    // once was" in their browse panel
    fn pseudo_live_versions(
        entries: Vec<BasicDirEntryInfo>,
        pseudo_live_dir: &Path,
    ) -> Vec<BasicDirEntryInfo> {
        entries
            .into_iter()
            .map(|basic_info| {
                BasicDirEntryInfo::new(
                    pseudo_live_dir.join(basic_info.path().file_name().unwrap_or_default()),
                    *basic_info.opt_filetype(),
                )
            })
            .collect()
    }

    fn display_or_transmit(
        entries: Vec<BasicDirEntryInfo>,
        is_phantom: PathProvenance,
        skim_tx: &SkimItemSender,
    ) -> HttmResult<()> {
        // send to the interactive view, or print directly, never return back
        match &GLOBAL_CONFIG.exec_mode {
            ExecMode::Interactive(_) => Self::transmit(entries, is_phantom, skim_tx)?,
            ExecMode::NonInteractiveRecursive(progress_bar) => {
                if entries.is_empty() {
                    if GLOBAL_CONFIG.opt_recursive {
                        progress_bar.tick();
                    } else {
                        eprintln!(
              "NOTICE: httm could not find any deleted files at this directory level.  \
                        Perhaps try specifying a deleted mode in combination with \"--recursive\"."
            )
                    }
                } else {
                    NonInteractiveRecursiveWrapper::print(entries)?;

                    // keeps spinner from squashing last line of output
                    if GLOBAL_CONFIG.opt_recursive {
                        eprintln!();
                    }
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    fn transmit(
        entries: Vec<BasicDirEntryInfo>,
        is_phantom: PathProvenance,
        skim_tx: &SkimItemSender,
    ) -> HttmResult<()> {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        entries
            .into_iter()
            .try_for_each(|basic_info| {
                skim_tx.try_send(Arc::new(basic_info.into_selection(&is_phantom)))
            })
            .map_err(std::convert::Into::into)
    }
}

// this is wrapper for non-interactive searches, which will be executed through the SharedRecursive fns
// here we disable the skim transmitter, etc., because we will simply be printing anything we find
pub struct NonInteractiveRecursiveWrapper;

impl NonInteractiveRecursiveWrapper {
    #[allow(unused_variables)]
    pub fn exec() -> HttmResult<()> {
        // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
        let (dummy_skim_tx, _): (SkimItemSender, SkimItemReceiver) = unbounded();
        let started = Arc::new(AtomicBool::new(true));
        let hangup = Arc::new(AtomicBool::new(false));

        match &GLOBAL_CONFIG.opt_requested_dir {
            Some(requested_dir) => {
                RecursiveSearch::exec(requested_dir, dummy_skim_tx, hangup, started);
            }
            None => {
                return Err(HttmError::new(
                    "requested_dir should never be None in Display Recursive mode",
                )
                .into())
            }
        }

        Ok(())
    }

    fn print(entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
        let pseudo_live_set: Vec<PathData> = entries.into_iter().map(PathData::from).collect();

        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &pseudo_live_set)?;
        let output_buf = DisplayWrapper::from(&GLOBAL_CONFIG, versions_map).to_string();

        print_output_buf(&output_buf)
    }
}
