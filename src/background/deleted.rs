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

use crate::background::recursive::{PathProvenance, SharedRecursive};
use crate::config::generate::{DeletedMode, ExecMode};
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmResult;
use crate::lookup::deleted::DeletedFiles;
use crate::GLOBAL_CONFIG;
use rayon::Scope;
use skim::prelude::*;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub struct DeletedSearch {
    requested_dir: BasicDirEntryInfo,
    skim_tx: SkimItemSender,
    hangup: Arc<AtomicBool>,
}

impl DeletedSearch {
    // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
    pub fn spawn(
        requested_dir: &Path,
        deleted_scope: &Scope,
        skim_tx: &SkimItemSender,
        hangup: &Arc<AtomicBool>,
    ) {
        let new = Self::new(requested_dir, skim_tx.clone(), hangup.clone());

        deleted_scope.spawn(move |inner| {
            let _ = new.run_loop(inner);
        })
    }

    fn new(requested_dir: &Path, skim_tx: SkimItemSender, hangup: Arc<AtomicBool>) -> Self {
        Self {
            requested_dir: BasicDirEntryInfo::new(requested_dir.to_path_buf(), None),
            skim_tx,
            hangup,
        }
    }

    fn run_loop(&self, inner: &Scope) -> HttmResult<()> {
        let mut queue = vec![self.requested_dir.clone()];

        while let Some(deleted_dir) = queue.pop() {
            // check -- should deleted threads keep working?
            // exit/error on disconnected channel, which closes
            // at end of browse scope
            if self.hangup.load(Ordering::Relaxed) {
                break;
            }

            if let Ok(mut res) = self.enter_directory(&deleted_dir.path()) {
                match GLOBAL_CONFIG.exec_mode {
                    ExecMode::Interactive(_) => {
                        queue.append(&mut res);
                    }
                    ExecMode::NonInteractiveRecursive(_) => res.iter().for_each(|dir| {
                        Self::spawn(dir.path(), inner, &self.skim_tx, &self.hangup)
                    }),
                    _ => unreachable!(),
                }
            }
        }

        Ok(())
    }

    // deleted file search for all modes
    fn enter_directory(&self, requested_dir: &Path) -> HttmResult<Vec<BasicDirEntryInfo>> {
        // check -- should deleted threads keep working?
        // exit/error on disconnected channel, which closes
        // at end of browse scope
        if self.hangup.as_ref().load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }

        // obtain all unique deleted, unordered, unsorted, will need to fix
        let vec_deleted = DeletedFiles::new(&requested_dir)?.into_inner();

        if vec_deleted.is_empty() {
            return Ok(Vec::new());
        }

        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            vec_deleted.into_iter().partition(|entry| {
                // no need to traverse symlinks in deleted search
                SharedRecursive::is_entry_dir(entry)
            });

        SharedRecursive::combine_and_send_entries(
            vec_files,
            &vec_dirs,
            PathProvenance::IsPhantom,
            &requested_dir,
            &self.skim_tx,
        )?;

        // disable behind deleted dirs with DepthOfOne,
        // otherwise recurse and find all those deleted files
        //
        // don't propagate errors, errors we are most concerned about
        // are transmission errors, which are handled elsewhere
        if GLOBAL_CONFIG.opt_deleted_mode != Some(DeletedMode::DepthOfOne)
            && GLOBAL_CONFIG.opt_recursive
        {
            return Ok(vec_dirs);
        }

        Ok(Vec::new())
    }
}
