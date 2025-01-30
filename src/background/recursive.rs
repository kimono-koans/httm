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
use crate::data::selection::SelectionCandidate;
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::print_output_buf;
use crate::lookup::deleted::DeletedFiles;
use crate::{VersionsMap, GLOBAL_CONFIG};
use skim::prelude::*;
use std::fs::read_dir;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[derive(Clone, Copy)]
pub enum PathProvenance {
    FromLiveDataset,
    IsPhantom,
}

pub struct RecursiveSearch<'a> {
    requested_dir: &'a Path,
    skim_tx: SkimItemSender,
    hangup: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
    rt: Runtime,
}

impl<'a> RecursiveSearch<'a> {
    pub fn new(
        requested_dir: &'a Path,
        skim_tx: SkimItemSender,
        hangup: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    ) -> Self {
        let rt = Runtime::new().unwrap();

        Self {
            requested_dir,
            skim_tx,
            hangup,
            started,
            rt,
        }
    }

    pub fn exec(&self) {
        self.rt.block_on(async {
            self.run_loop();
        })
    }

    fn run_loop(&self) {
        // this runs the main loop for live file searches, see the referenced struct below
        // we are in our own detached system thread, so print error and exit if error trickles up
        self.loop_body().unwrap_or_else(|error| {
            eprintln!("ERROR: {error}");
            std::process::exit(1)
        });
    }

    fn loop_body(&self) -> HttmResult<()> {
        // the user may specify a dir for browsing,
        // but wants to restore that directory,
        // so here we add the directory and its parent as a selection item

        let handle = self.rt.handle();

        let dot_as_entry = BasicDirEntryInfo::new(
            self.requested_dir.to_path_buf(),
            Some(self.requested_dir.metadata()?.file_type()),
        );

        let mut initial_vec_dirs = vec![dot_as_entry];

        if let Some(parent) = self.requested_dir.parent() {
            let double_dot_as_entry =
                BasicDirEntryInfo::new(parent.to_path_buf(), Some(parent.metadata()?.file_type()));

            initial_vec_dirs.push(double_dot_as_entry)
        }

        let initial_entries = Entries {
            path_provenance: &PathProvenance::FromLiveDataset,
            skim_tx: &self.skim_tx,
            vec_dirs: initial_vec_dirs,
            vec_files: Vec::new(),
        };

        initial_entries.combine_and_send()?;

        // runs once for non-recursive but also "primes the pump"
        // for recursive to have items available, also only place an
        // error can stop execution
        let mut queue: Vec<BasicDirEntryInfo> = Self::enter_directory(
            &self.requested_dir,
            &self.skim_tx,
            &self.hangup,
            &PathProvenance::FromLiveDataset,
        )?;

        let mut deleted_join_handles = Vec::new();

        if GLOBAL_CONFIG.opt_deleted_mode.is_some() {
            // Spawn a future onto the runtime
            let request = self.requested_dir.to_path_buf();

            let skim_tx = self.skim_tx.clone();

            let hangup = self.hangup.clone();

            deleted_join_handles
                .push(handle.spawn_blocking(|| DeletedSearch::run_loop(request, skim_tx, hangup)));
        }

        self.started.store(true, Ordering::SeqCst);

        if GLOBAL_CONFIG.opt_recursive {
            // condition kills iter when user has made a selection
            // pop_back makes this a LIFO queue which is supposedly better for caches
            while let Some(item) = queue.pop() {
                // check -- should deleted threads keep working?
                // exit/error on disconnected channel, which closes
                // at end of browse scope
                if self.hangup.load(Ordering::Relaxed) {
                    break;
                }

                if GLOBAL_CONFIG.opt_deleted_mode.is_some() {
                    // Spawn a future onto the runtime
                    let request = item.clone().to_path_buf();

                    let skim_tx = self.skim_tx.clone();

                    let hangup = self.hangup.clone();

                    deleted_join_handles.push(
                        handle.spawn_blocking(|| DeletedSearch::run_loop(request, skim_tx, hangup)),
                    );
                }

                // no errors will be propagated in recursive mode
                // far too likely to run into a dir we don't have permissions to view
                if let Ok(mut items) = Self::enter_directory(
                    &item.path(),
                    &self.skim_tx,
                    &self.hangup,
                    &PathProvenance::FromLiveDataset,
                ) {
                    queue.append(&mut items)
                }
            }
        }

        while deleted_join_handles
            .iter()
            .any(|handle| !handle.is_finished())
        {}

        Ok(())
    }

    // deleted file search for all modes
    #[inline(always)]
    pub fn enter_directory(
        requested_dir: &Path,
        skim_tx: &SkimItemSender,
        hangup: &Arc<AtomicBool>,
        path_provenance: &PathProvenance,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        // check -- should deleted threads keep working?
        // exit/error on disconnected channel, which closes
        // at end of browse scope
        if hangup.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }

        // create entries struct here
        let entries = Entries::new(requested_dir, &path_provenance, &skim_tx)?;

        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let vec_dirs = entries.combine_and_send()?;

        // disable behind deleted dirs with DepthOfOne,
        // otherwise recurse and find all those deleted files
        //
        // don't propagate errors, errors we are most concerned about
        // are transmission errors, which are handled elsewhere
        if let Some(DeletedMode::DepthOfOne) = GLOBAL_CONFIG.opt_deleted_mode {
            return Ok(Vec::new());
        }

        Ok(vec_dirs)
    }
}

pub struct Entries<'a> {
    pub path_provenance: &'a PathProvenance,
    pub skim_tx: &'a SkimItemSender,
    pub vec_dirs: Vec<BasicDirEntryInfo>,
    pub vec_files: Vec<BasicDirEntryInfo>,
}

impl<'a> Entries<'a> {
    #[inline(always)]
    pub fn new(
        requested_dir: &'a Path,
        path_provenance: &'a PathProvenance,
        skim_tx: &'a SkimItemSender,
    ) -> HttmResult<Self> {
        // separates entries into dirs and files
        let (vec_dirs, vec_files) = match path_provenance {
            PathProvenance::FromLiveDataset => {
                read_dir(requested_dir)?
                    .flatten()
                    // checking file_type on dir entries is always preferable
                    // as it is much faster than a metadata call on the path
                    .map(|dir_entry| BasicDirEntryInfo::from(dir_entry))
                    .filter(|entry| entry.all_exclusions())
                    .partition(|entry| entry.is_entry_dir())
            }
            PathProvenance::IsPhantom => {
                // obtain all unique deleted, unordered, unsorted, will need to fix
                DeletedFiles::new(&requested_dir)?
                    .into_inner()
                    .into_iter()
                    .filter(|entry| entry.all_exclusions())
                    .partition(|entry| entry.is_entry_dir())
            }
        };

        Ok(Self {
            path_provenance,
            skim_tx,
            vec_dirs,
            vec_files,
        })
    }

    #[inline(always)]
    pub fn combine_and_send(self) -> HttmResult<Vec<BasicDirEntryInfo>> {
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
                let mut combined = self.vec_files;
                combined.extend_from_slice(&self.vec_dirs);
                combined
            }
        };

        DisplayOrTransmit::new(entries_ready_to_send, self.path_provenance, self.skim_tx).exec()?;

        // here we consume the struct after sending the entries,
        // however we still need the dirs to populate the loop's queue
        // so we return the vec of dirs here
        Ok(self.vec_dirs)
    }
}

struct DisplayOrTransmit<'a> {
    combined_entries: Vec<BasicDirEntryInfo>,
    path_provenance: &'a PathProvenance,
    skim_tx: &'a SkimItemSender,
}

impl<'a> DisplayOrTransmit<'a> {
    fn new(
        combined_entries: Vec<BasicDirEntryInfo>,
        path_provenance: &'a PathProvenance,
        skim_tx: &'a SkimItemSender,
    ) -> Self {
        Self {
            combined_entries,
            path_provenance,
            skim_tx,
        }
    }

    fn exec(self) -> HttmResult<()> {
        // send to the interactive view, or print directly, never return back
        match &GLOBAL_CONFIG.exec_mode {
            ExecMode::Interactive(_) => self.transmit()?,
            ExecMode::NonInteractiveRecursive(progress_bar) if self.combined_entries.is_empty() => {
                if GLOBAL_CONFIG.opt_recursive {
                    progress_bar.tick();
                } else {
                    eprintln!(
                        "NOTICE: httm could not find any deleted files at this directory level.  \
                        Perhaps try specifying a deleted mode in combination with \"--recursive\"."
                    )
                }
            }
            ExecMode::NonInteractiveRecursive(_) => {
                self.display()?;

                // keeps spinner from squashing last line of output
                if GLOBAL_CONFIG.opt_recursive {
                    eprintln!();
                }
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    fn transmit(self) -> HttmResult<()> {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        self.combined_entries
            .into_iter()
            .map(|basic_dir_entry_info| {
                SelectionCandidate::new(basic_dir_entry_info, &self.path_provenance)
            })
            .try_for_each(|selection_candidate| {
                self.skim_tx.try_send(Arc::new(selection_candidate))
            })
            .map_err(std::convert::Into::into)
    }

    fn display(self) -> HttmResult<()> {
        let pseudo_live_set: Vec<PathData> = self
            .combined_entries
            .into_iter()
            .map(PathData::from)
            .collect();

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
        let (dummy_skim_tx, _): (SkimItemSender, SkimItemReceiver) = unbounded();
        let started = Arc::new(AtomicBool::new(true));
        let hangup = Arc::new(AtomicBool::new(false));

        match &GLOBAL_CONFIG.opt_requested_dir {
            Some(requested_dir) => {
                RecursiveSearch::new(requested_dir, dummy_skim_tx, hangup, started).exec();
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
}
