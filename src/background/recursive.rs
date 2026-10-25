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
use crate::data::selection::SelectionCandidate;
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
use std::fs::read_dir;
use std::hash::Hash;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc,
    LazyLock,
};

#[derive(Clone, Copy)]
pub enum PathProvenance {
    FromLiveDataset,
    IsPhantom,
}

pub struct EntriesPartitioned {
    vec_dirs: Vec<BasicDirEntryInfo>,
    vec_files: Vec<BasicDirEntryInfo>,
}

impl EntriesPartitioned {
    fn new(common_search: &mut impl CommonSearch, requested_dir: &Path) -> HttmResult<Self> {
        // separates entries into dirs and files
        let (vec_dirs, vec_files) = match common_search.path_provenance() {
            PathProvenance::FromLiveDataset => {
                read_dir(requested_dir)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(dir_entry))
                    .filter(|entry: &BasicDirEntryInfo| entry.recursive_search_filter())
                    .partition(|entry| common_search.entry_is_dir(entry))
            }
            PathProvenance::IsPhantom => {
                // obtain all unique deleted, unordered, unsorted, will need to fix
                DeletedFiles::new(requested_dir)
                    .into_inner()
                    .into_iter()
                    .partition(|pseudo_entry| common_search.entry_is_dir(pseudo_entry))
            }
        };

        Ok(EntriesPartitioned {
            vec_dirs,
            vec_files,
        })
    }

    fn combine_into_iter(self) -> impl Iterator<Item = BasicDirEntryInfo> {
        self.vec_files.into_iter().chain(self.vec_dirs.into_iter())
    }

    fn is_empty(&self) -> bool {
        self.vec_dirs.is_empty() && self.vec_files.is_empty()
    }

    fn transmit(self, common_search: &impl CommonSearch) -> HttmResult<()> {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        let vec = self
            .combine_into_iter()
            .map(|basic_dir_entry_info| {
                let item: Arc<dyn SkimItem> =
                    Arc::new(SelectionCandidate::from(basic_dir_entry_info));

                item
            })
            .collect();

        common_search
            .opt_sender()
            .expect("Sender must be Some in any interactive mode")
            .send(vec)
            .map_err(std::convert::Into::into)
    }

    fn display(self) -> HttmResult<()> {
        let pseudo_live_set: Vec<PathData> = self.combine_into_iter().map(PathData::from).collect();

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

pub struct RecursiveSearch<'a> {
    opt_skim_tx: Option<&'a SkimItemSender>,
    hangup: Arc<AtomicBool>,
    not_previously_displayed_cache: HashSet<UniqueInode>,
}

impl<'a> RecursiveSearch<'a> {
    pub fn new(opt_skim_tx: Option<&'a SkimItemSender>, hangup: Arc<AtomicBool>) -> Self {
        let not_previously_displayed_cache: HashSet<UniqueInode> = HashSet::new();

        Self {
            opt_skim_tx,
            hangup,
            not_previously_displayed_cache,
        }
    }

    pub fn exec(&mut self, requested_dir: &'a Path) {
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
                    self.run_loop(requested_dir, Some(deleted_scope))
                        .unwrap_or_else(|err| exit_error(err));
                })
            }
            None => {
                self.run_loop(requested_dir, None)
                    .unwrap_or_else(|err| exit_error(err));
            }
        }
    }

    fn spawn_deleted_search(&self, requested_dir: &'a Path, deleted_scope: &Scope<'_>) {
        let search = DeletedSearch::new(self.opt_skim_tx.cloned(), self.hangup.clone());

        search.spawn(requested_dir, deleted_scope);
    }

    fn add_dot_entries(&'a self, requested_dir: &'a Path) -> HttmResult<()> {
        let dot_as_entry = BasicDirEntryInfo::new(requested_dir, None);

        let mut initial_vec_dirs = vec![dot_as_entry];

        if let Some(parent) = requested_dir.parent() {
            let double_dot_as_entry = BasicDirEntryInfo::new(parent, None);

            initial_vec_dirs.push(double_dot_as_entry)
        }

        let entries = EntriesPartitioned {
            vec_dirs: initial_vec_dirs,
            vec_files: Vec::new(),
        };

        self.display_or_transmit(entries)?;

        Ok(())
    }

    fn entry_not_previously_displayed(&mut self, entry: &BasicDirEntryInfo) -> bool {
        let Some(file_id) = UniqueInode::new(entry) else {
            return false;
        };

        self.not_previously_displayed_cache.insert(file_id)
    }
}

impl CommonSearch for RecursiveSearch<'_> {
    fn hangup(&self) -> bool {
        self.hangup.load(Ordering::Relaxed)
    }

    fn entry_is_dir(&mut self, entry: &BasicDirEntryInfo) -> bool {
        // must do is_dir() look up on DirEntry file_type() as look up on Path will traverse links!
        if GLOBAL_CONFIG.opt_no_traverse {
            if let Some(file_type) = entry.opt_filetype() {
                return file_type.is_dir();
            }
        }

        entry.httm_is_dir::<BasicDirEntryInfo>() && self.entry_not_previously_displayed(entry)
    }

    fn run_loop(
        &mut self,
        requested_dir: &Path,
        opt_deleted_scope: Option<&Scope>,
    ) -> HttmResult<()> {
        // the user may specify a dir for browsing,
        // but wants to restore that directory,
        // so here we add the directory and its parent as a selection item
        self.add_dot_entries(requested_dir)?;

        // runs once for non-recursive but also "primes the pump"
        // for recursive to have items available, also only place an
        // error can stop execution
        let mut queue: Vec<BasicDirEntryInfo> = Vec::new();

        self.enter_directory(requested_dir, &mut queue)?;

        if let Some(deleted_scope) = opt_deleted_scope {
            self.spawn_deleted_search(requested_dir, deleted_scope);
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
    fn entry_is_dir(&mut self, basic_dir_entry: &BasicDirEntryInfo) -> bool;
    fn run_loop(
        &mut self,
        requested_dir: &Path,
        opt_deleted_scope: Option<&Scope>,
    ) -> HttmResult<()>;
    fn opt_sender(&self) -> Option<&SkimItemSender>;
    fn path_provenance(&self) -> PathProvenance;

    // deleted file search for all modes
    #[inline(always)]
    fn enter_directory(
        &mut self,
        requested_dir: &Path,
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
        let entries = EntriesPartitioned::new(self, requested_dir)?;

        // append dirs to queue
        queue.extend_from_slice(&entries.vec_dirs);

        // combined entries will be sent or printed
        self.display_or_transmit(entries)?;

        Ok(())
    }

    fn display_or_transmit(&self, entries: EntriesPartitioned) -> HttmResult<()>
    where
        Self: Sized,
    {
        if let PathProvenance::FromLiveDataset = self.path_provenance() {
            // no need to display or transmit live entries in deleted only mode or non-interactice recursive
            static DELETED_ONLY_OR_NON_INTERACTIVE_RECURSIVE: LazyLock<bool> =
                LazyLock::new(|| {
                    matches!(GLOBAL_CONFIG.opt_deleted_mode, Some(DeletedMode::Only))
                        || matches!(
                            GLOBAL_CONFIG.exec_mode,
                            ExecMode::NonInteractiveRecursive(_)
                        )
                });

            if *DELETED_ONLY_OR_NON_INTERACTIVE_RECURSIVE {
                return Ok(());
            }
        }

        // send to the interactive view, or print directly, never return back
        match &GLOBAL_CONFIG.exec_mode {
            ExecMode::Interactive(_) => entries.transmit(self)?,
            ExecMode::NonInteractiveRecursive(progress_bar) if entries.is_empty() => {
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
                entries.display()?;

                // keeps spinner from squashing last line of output
                if GLOBAL_CONFIG.opt_recursive {
                    eprintln!();
                }
            }
            _ => unreachable!(),
        }

        Ok(())
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
                RecursiveSearch::new(opt_skim_tx, hangup).exec(&requested_dir);
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
