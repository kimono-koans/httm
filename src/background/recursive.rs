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
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::UniqueInode;
use crate::library::utility::print_output_buf;
use crate::lookup::deleted::DeletedFiles;
use crate::{GLOBAL_CONFIG, VersionsMap};
use hashbrown::HashSet;
use rayon::{Scope, ThreadPool};
use skim::SkimItem;
use skim::prelude::*;
use std::fs::read_dir;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub enum PathProvenance {
    FromLiveDataset,
    IsPhantom,
}

pub struct RecursiveSearch<'a> {
    requested_dir: &'a Path,
    opt_skim_tx: Option<&'a SkimItemSender>,
    hangup: Arc<AtomicBool>,
    path_map: Mutex<HashSet<UniqueInode>>,
}

impl<'a> RecursiveSearch<'a> {
    pub fn new(
        requested_dir: &'a Path,
        opt_skim_tx: Option<&'a SkimItemSender>,
        hangup: Arc<AtomicBool>,
    ) -> Self {
        let path_map: Mutex<HashSet<UniqueInode>> = Mutex::new(HashSet::new());

        Self {
            requested_dir,
            opt_skim_tx,
            hangup,
            path_map,
        }
    }

    pub fn exec(&self) {
        if GLOBAL_CONFIG.opt_deleted_mode.is_some() {
            // thread pool allows deleted to have its own scope, which means
            // all threads must complete before the scope exits.  this is important
            // for display recursive searches as the live enumeration will end before
            // all deleted threads have completed
            let pool: ThreadPool = rayon::ThreadPoolBuilder::new()
                .build()
                .expect("Could not initialize rayon thread pool for recursive deleted search");

            pool.scope(|deleted_scope| {
                self.run_loop(Some(deleted_scope));
            })
        } else {
            self.run_loop(None);
        }
    }

    fn run_loop(&self, opt_deleted_scope: Option<&Scope>) {
        // this runs the main loop for live file searches, see the referenced struct below
        // we are in our own detached system thread, so print error and exit if error trickles up
        self.loop_body(opt_deleted_scope).unwrap_or_else(|error| {
            eprintln!("ERROR: {error}");
            std::process::exit(1)
        });
    }

    fn loop_body(&self, opt_deleted_scope: Option<&Scope>) -> HttmResult<()> {
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
            let mut total_items = 0;

            let interactive = !matches!(
                GLOBAL_CONFIG.exec_mode,
                ExecMode::NonInteractiveRecursive(_)
            );

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

                total_items += 1;

                if interactive && total_items % 100 == 0 {
                    std::thread::yield_now();
                }
            }
        }

        Ok(())
    }

    fn spawn_deleted_search(&self, requested_dir: &'a Path, deleted_scope: &Scope<'_>) {
        DeletedSearch::spawn(
            requested_dir,
            deleted_scope,
            self.opt_skim_tx.cloned(),
            self.hangup.clone(),
        )
    }

    fn add_dot_entries(&self) -> HttmResult<()> {
        let dot_as_entry = BasicDirEntryInfo::new(self.requested_dir, None);

        let mut initial_vec_dirs = vec![dot_as_entry];

        if let Some(parent) = self.requested_dir.parent() {
            let double_dot_as_entry = BasicDirEntryInfo::new(parent, None);

            initial_vec_dirs.push(double_dot_as_entry)
        }

        let entries = Entries {
            requested_dir: self.requested_dir,
            path_provenance: &PathProvenance::FromLiveDataset,
            opt_skim_tx: self.opt_skim_tx,
        };

        let paths_partitioned = PathsPartitioned {
            vec_dirs: initial_vec_dirs,
            vec_files: Vec::new(),
        };

        entries.combine_and_deliver(paths_partitioned)?;

        Ok(())
    }
}

pub trait CommonSearch {
    fn hangup(&self) -> bool;
    fn opt_path_map(&self) -> Option<&Mutex<HashSet<UniqueInode>>>;
    fn into_entries<'a>(&'a self, requested_dir: &'a Path) -> Entries<'a>;
    fn enter_directory(
        &self,
        requested_dir: &Path,
        queue: &mut Vec<BasicDirEntryInfo>,
    ) -> HttmResult<()>;
}

impl CommonSearch for &RecursiveSearch<'_> {
    fn enter_directory(
        &self,
        requested_dir: &Path,
        queue: &mut Vec<BasicDirEntryInfo>,
    ) -> HttmResult<()> {
        enter_directory(self, requested_dir, queue)
    }

    fn hangup(&self) -> bool {
        self.hangup.load(Ordering::Relaxed)
    }

    fn opt_path_map(&self) -> Option<&Mutex<HashSet<UniqueInode>>> {
        Some(&self.path_map)
    }

    fn into_entries<'a>(&'a self, requested_dir: &'a Path) -> Entries<'a> {
        Entries {
            requested_dir,
            path_provenance: &PathProvenance::FromLiveDataset,
            opt_skim_tx: self.opt_skim_tx,
        }
    }
}

// deleted file search for all modes
#[inline(always)]
pub fn enter_directory<'a, T>(
    search: &T,
    requested_dir: &'a Path,
    queue: &mut Vec<BasicDirEntryInfo>,
) -> HttmResult<()>
where
    T: CommonSearch,
{
    // check -- should deleted threads keep working?
    // exit/error on disconnected channel, which closes
    // at end of browse scope
    if search.hangup() {
        return Ok(());
    }

    // create entries struct here
    let entries = search.into_entries(requested_dir);

    let paths_partitioned = PathsPartitioned::new(&entries, search.opt_path_map())?;

    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let mut vec_dirs = entries.combine_and_deliver(paths_partitioned)?;

    queue.append(&mut vec_dirs);

    Ok(())
}

struct PathsPartitioned {
    vec_dirs: Vec<BasicDirEntryInfo>,
    vec_files: Vec<BasicDirEntryInfo>,
}

impl PathsPartitioned {
    fn new(
        entries: &Entries,
        opt_path_map: Option<&Mutex<HashSet<UniqueInode>>>,
    ) -> HttmResult<PathsPartitioned> {
        // separates entries into dirs and files
        let (vec_dirs, vec_files) = match entries.path_provenance {
            PathProvenance::FromLiveDataset => {
                read_dir(entries.requested_dir)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(dir_entry))
                    .filter(|entry| entry.recursive_search_filter())
                    .partition(|entry| entry.is_entry_dir(opt_path_map))
            }
            PathProvenance::IsPhantom => {
                // obtain all unique deleted, unordered, unsorted, will need to fix
                DeletedFiles::from(entries.requested_dir)
                    .into_inner()
                    .into_iter()
                    .partition(|pseudo_entry| {
                        pseudo_entry
                            .opt_filetype()
                            .map(|file_type| file_type.is_dir())
                            .unwrap_or_else(|| false)
                    })
            }
        };

        Ok(Self {
            vec_dirs,
            vec_files,
        })
    }
}

pub struct Entries<'a> {
    requested_dir: &'a Path,
    path_provenance: &'a PathProvenance,
    opt_skim_tx: Option<&'a SkimItemSender>,
}

impl<'a> Entries<'a> {
    #[inline(always)]
    pub fn new(
        requested_dir: &'a Path,
        path_provenance: &'a PathProvenance,
        opt_skim_tx: Option<&'a SkimItemSender>,
    ) -> Self {
        Self {
            requested_dir,
            path_provenance,
            opt_skim_tx,
        }
    }

    #[inline(always)]
    fn combine_and_deliver(
        &self,
        paths_partitioned: PathsPartitioned,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        let entries_ready_to_send = match self.path_provenance {
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
                let mut combined = paths_partitioned.vec_files;

                combined.extend_from_slice(&paths_partitioned.vec_dirs);
                combined
            }
        };

        self.display_or_transmit(entries_ready_to_send)?;

        // here we consume the struct after sending the entries,
        // however we still need the dirs to populate the loop's queue
        // so we return the vec of dirs here
        Ok(paths_partitioned.vec_dirs)
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
                self.display(combined_entries)?;

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
                let item: Arc<dyn SkimItem> =
                    Arc::new(basic_dir_entry_info.into_selection_candidate(self.path_provenance));

                item
            })
            .collect();

        self.opt_skim_tx
            .expect("Sender must be Some in any interactive mode")
            .send(vec)
            .map_err(std::convert::Into::into)
    }

    fn display(&self, combined_entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
        let pseudo_live_set: Vec<PathData> =
            combined_entries.into_iter().map(PathData::from).collect();

        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &pseudo_live_set)?;
        let output_buf = DisplayWrapper::from(&GLOBAL_CONFIG, versions_map).to_string();

        print_output_buf(&output_buf)
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
