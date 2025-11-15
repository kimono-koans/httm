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
use crate::config::generate::{
    DeletedMode,
    ExecMode,
};
use crate::data::paths::{
    BasicDirEntryInfo,
    PathData,
};
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::{
    HttmError,
    HttmResult,
};
use crate::library::utility::{
    HttmIsDir,
    print_output_buf,
};
use crate::lookup::deleted::DeletedFiles;
use crate::{
    GLOBAL_CONFIG,
    VersionsMap,
    exit_error,
};
use hashbrown::HashSet;
use rayon::{
    Scope,
    ThreadPool,
};
use skim::SkimItem;
use skim::prelude::*;
use std::cell::RefCell;
use std::fs::read_dir;
use std::hash::Hash;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
pub enum PathProvenance {
    FromLiveDataset,
    IsPhantom,
}

pub struct EntriesPartitioned {
    vec_dirs: Vec<BasicDirEntryInfo>,
    vec_files: Vec<BasicDirEntryInfo>,
}

pub struct RecursiveSearch<'a> {
    requested_dir: &'a Path,
    opt_skim_tx: Option<&'a SkimItemSender>,
    hangup: Arc<AtomicBool>,
    not_previously_displayed_cache: RefCell<HashSet<UniqueInode>>,
}

impl<'a> RecursiveSearch<'a> {
    pub fn new(
        requested_dir: &'a Path,
        opt_skim_tx: Option<&'a SkimItemSender>,
        hangup: Arc<AtomicBool>,
    ) -> Self {
        let not_previously_displayed_cache: RefCell<HashSet<UniqueInode>> =
            RefCell::new(HashSet::new());

        Self {
            requested_dir,
            opt_skim_tx,
            hangup,
            not_previously_displayed_cache,
        }
    }

    pub fn exec(&self) {
        match GLOBAL_CONFIG.opt_deleted_mode {
            Some(_) => {
                // thread pool allows deleted to have its own scope, which means
                // all threads must complete before the scope exits.  this is important
                // for display recursive searches as the live enumeration will end before
                // all deleted threads have completed
                let pool: ThreadPool = rayon::ThreadPoolBuilder::new()
                    .build()
                    .expect("Could not initialize rayon thread pool for recursive deleted search");

                pool.in_place_scope(|deleted_scope| {
                    self.run_loop(Some(deleted_scope))
                        .unwrap_or_else(|err| exit_error(err));
                })
            }
            None => {
                self.run_loop(None).unwrap_or_else(|err| exit_error(err));
            }
        }
    }

    fn spawn_deleted_search(&self, requested_dir: &'a Path, deleted_scope: &Scope<'_>) {
        DeletedSearch::spawn(
            requested_dir,
            deleted_scope,
            self.opt_skim_tx.cloned(),
            self.hangup.clone(),
        )
    }

    fn add_dot_entries(&'a self) -> HttmResult<()> {
        let dot_as_entry = BasicDirEntryInfo::new(self.requested_dir, None);

        let mut initial_vec_dirs = vec![dot_as_entry];

        if let Some(parent) = self.requested_dir.parent() {
            let double_dot_as_entry = BasicDirEntryInfo::new(parent, None);

            initial_vec_dirs.push(double_dot_as_entry)
        }

        let entries = EntriesPartitioned {
            vec_dirs: initial_vec_dirs,
            vec_files: Vec::new(),
        };

        self.combine_and_deliver_entries(entries)?;

        Ok(())
    }

    fn entry_not_previously_displayed(&self, entry: &BasicDirEntryInfo) -> bool {
        let Some(file_id) = UniqueInode::new(entry) else {
            return false;
        };

        let mut write_locked = self.not_previously_displayed_cache.borrow_mut();

        write_locked.insert(file_id)
    }
}

impl CommonSearch for RecursiveSearch<'_> {
    fn hangup(&self) -> bool {
        self.hangup.load(Ordering::Relaxed)
    }

    fn entry_is_dir(&self, entry: &BasicDirEntryInfo) -> bool {
        // must do is_dir() look up on DirEntry file_type() as look up on Path will traverse links!
        if GLOBAL_CONFIG.opt_no_traverse {
            if let Some(file_type) = entry.opt_filetype() {
                return file_type.is_dir();
            }
        }

        entry.httm_is_dir::<BasicDirEntryInfo>() && self.entry_not_previously_displayed(entry)
    }

    fn run_loop(&self, opt_deleted_scope: Option<&Scope>) -> HttmResult<()> {
        // the user may specify a dir for browsing,
        // but wants to restore that directory,
        // so here we add the directory and its parent as a selection item
        self.add_dot_entries()?;

        // runs once for non-recursive but also "primes the pump"
        // for recursive to have items available, also only place an
        // error can stop execution
        let mut queue: Vec<BasicDirEntryInfo> = Vec::new();

        self.enter_directory(self.requested_dir, &mut queue)?;

        if let Some(deleted_scope) = opt_deleted_scope {
            self.spawn_deleted_search(&self.requested_dir, deleted_scope);
        }

        if GLOBAL_CONFIG.opt_recursive {
            // condition kills iter when user has made a selection
            // pop_back makes this a LIFO queue which is supposedly better for caches
            while let Some(item) = queue.pop() {
                // check -- should deleted threads keep working?
                // exit/error on disconnected channel, which closes
                // at end of browse scope
                if self.hangup() {
                    break;
                }

                if let Some(deleted_scope) = opt_deleted_scope {
                    self.spawn_deleted_search(&item.path(), deleted_scope);
                }

                // no errors will be propagated in recursive mode
                // far too likely to run into a dir we don't have permissions to view
                let _ = self.enter_directory(item.path(), &mut queue);
            }
        }

        Ok(())
    }

    fn opt_sender(&self) -> Option<&SkimItemSender> {
        self.opt_skim_tx
    }

    fn path_provenance(&self) -> PathProvenance {
        PathProvenance::FromLiveDataset
    }
}

pub trait CommonSearch {
    fn hangup(&self) -> bool;
    fn entry_is_dir(&self, basic_dir_entry: &BasicDirEntryInfo) -> bool;
    fn run_loop(&self, opt_deleted_scope: Option<&Scope>) -> HttmResult<()>;
    fn opt_sender(&self) -> Option<&SkimItemSender>;
    fn path_provenance(&self) -> PathProvenance;

    // deleted file search for all modes
    #[inline(always)]
    fn enter_directory<'a>(
        &self,
        requested_dir: &'a Path,
        queue: &mut Vec<BasicDirEntryInfo>,
    ) -> HttmResult<()>
    where
        Self: Sized,
    {
        // check -- should deleted threads keep working?
        // exit/error on disconnected channel, which closes
        // at end of browse scope
        if self.hangup() {
            return Ok(());
        }

        // create entries struct here
        let entries = self.entries_partitioned(requested_dir)?;

        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let mut vec_dirs = self.combine_and_deliver_entries(entries)?;

        queue.append(&mut vec_dirs);

        Ok(())
    }

    fn entries_partitioned(&self, requested_dir: &Path) -> HttmResult<EntriesPartitioned> {
        // separates entries into dirs and files
        let (vec_dirs, vec_files) = match self.path_provenance() {
            PathProvenance::FromLiveDataset => {
                read_dir(requested_dir)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(dir_entry))
                    .filter(|entry: &BasicDirEntryInfo| entry.recursive_search_filter())
                    .partition(|entry| self.entry_is_dir(entry))
            }
            PathProvenance::IsPhantom => {
                // obtain all unique deleted, unordered, unsorted, will need to fix
                DeletedFiles::new(requested_dir)
                    .into_inner()
                    .into_iter()
                    .partition(|pseudo_entry| self.entry_is_dir(pseudo_entry))
            }
        };

        Ok(EntriesPartitioned {
            vec_dirs,
            vec_files,
        })
    }

    #[inline(always)]
    fn combine_and_deliver_entries(
        &self,
        entries: EntriesPartitioned,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        let EntriesPartitioned {
            vec_dirs,
            vec_files,
        } = entries;

        let entries_ready_to_send = match self.path_provenance() {
            PathProvenance::FromLiveDataset
                if matches!(GLOBAL_CONFIG.opt_deleted_mode, Some(DeletedMode::Only))
                    || matches!(
                        GLOBAL_CONFIG.exec_mode,
                        ExecMode::NonInteractiveRecursive(_)
                    ) =>
            {
                Vec::new()
            }
            PathProvenance::FromLiveDataset | PathProvenance::IsPhantom => {
                let mut combined = vec_files;

                combined.extend_from_slice(&vec_dirs);
                combined
            }
        };

        self.display_or_transmit(entries_ready_to_send)?;

        // here we consume the struct after sending the entries,
        // however we still need the dirs to populate the loop's queue
        // so we return the vec of dirs here
        Ok(vec_dirs)
    }

    fn display_or_transmit(&self, combined_entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
        // send to the interactive view, or print directly, never return back
        match &GLOBAL_CONFIG.exec_mode {
            ExecMode::Interactive(_) => self.transmit(combined_entries)?,
            ExecMode::NonInteractiveRecursive(progress_bar) if combined_entries.is_empty() => {
                if !GLOBAL_CONFIG.opt_recursive {
                    eprintln!(
                        "NOTICE: httm could not find any deleted files at this directory level.  \
                        Perhaps try specifying a deleted mode in combination with \"--recursive\"."
                    );
                    return Ok(());
                }

                progress_bar.tick();
            }
            ExecMode::NonInteractiveRecursive(_) => {
                Self::display(combined_entries)?;

                // keeps spinner from squashing last line of output
                if GLOBAL_CONFIG.opt_recursive {
                    eprintln!();
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    fn transmit(&self, combined_entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        let vec: Vec<Arc<dyn SkimItem>> = combined_entries
            .into_iter()
            .map(|basic_dir_entry_info| {
                let item: Arc<dyn SkimItem> = Arc::new(
                    basic_dir_entry_info.into_selection_candidate(&self.path_provenance()),
                );

                item
            })
            .collect();

        self.opt_sender()
            .expect("Sender must be Some in any interactive mode")
            .send(vec)
            .map_err(std::convert::Into::into)
    }

    fn display(combined_entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
        let pseudo_live_set: Vec<PathData> =
            combined_entries.into_iter().map(PathData::from).collect();

        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &pseudo_live_set)?;
        let output_buf = DisplayWrapper::from(&GLOBAL_CONFIG, versions_map).to_string();

        print_output_buf(&output_buf)
    }
}

pub struct UniqueInode {
    ino: u64,
    dev: u64,
}

impl UniqueInode {
    fn new(entry: &BasicDirEntryInfo) -> Option<Self> {
        if entry.opt_filetype().is_some_and(|ft| ft.is_symlink()) {
            return entry.path().metadata().ok().map(|md| Self {
                ino: md.ino(),
                dev: md.dev(),
            });
        }

        entry.opt_metadata().map(|md| Self {
            ino: md.ino(),
            dev: md.dev(),
        })
    }
}

impl PartialEq for UniqueInode {
    fn eq(&self, other: &Self) -> bool {
        self.ino == other.ino && self.dev == other.dev
    }
}

impl Eq for UniqueInode {}

impl Hash for UniqueInode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ino.hash(state);
        self.dev.hash(state);
    }
}

// this is wrapper for non-interactive searches, which will be executed through the SharedRecursive fns
// here we disable the skim transmitter, etc., because we will simply be printing anything we find
pub struct NonInteractiveRecursiveWrapper;

impl NonInteractiveRecursiveWrapper {
    #[allow(unused_variables)]
    pub fn exec() -> HttmResult<()> {
        // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
        let opt_skim_tx = None;
        let hangup = Arc::new(AtomicBool::new(false));

        match &GLOBAL_CONFIG.opt_requested_dir {
            Some(requested_dir) => {
                RecursiveSearch::new(requested_dir, opt_skim_tx, hangup).exec();
            }
            None => {
                return HttmError::new(
                    "requested_dir should never be None in Display Recursive mode",
                )
                .into();
            }
        }

        Ok(())
    }
}
