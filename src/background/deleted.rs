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

use crate::background::recursive::PathProvenance;
use crate::background::recursive::RecursiveSearch;
use crate::config::generate::DeletedMode;
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmResult;
use crate::GLOBAL_CONFIG;
use rayon::Scope;
use skim::prelude::*;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub struct DeletedSearch;

impl DeletedSearch {
    // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
    pub fn spawn(
        requested_dir: &Path,
        deleted_scope: &Scope,
        skim_tx: SkimItemSender,
        hangup: Arc<AtomicBool>,
    ) {
        let deleted_dir = requested_dir.to_path_buf();

        deleted_scope.spawn(move |_| {
            let _ = Self::run_loop(deleted_dir, skim_tx.clone(), hangup.clone());
        })
    }

    fn run_loop(
        deleted_dir: PathBuf,
        skim_tx: SkimItemSender,
        hangup: Arc<AtomicBool>,
    ) -> HttmResult<()> {
        if hangup.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut queue: Vec<BasicDirEntryInfo> = RecursiveSearch::enter_directory(
            &deleted_dir,
            &skim_tx,
            &hangup,
            &PathProvenance::IsPhantom,
        )?;

        if matches!(
            GLOBAL_CONFIG.opt_deleted_mode,
            Some(DeletedMode::DepthOfOne)
        ) {
            return Ok(());
        }

        if GLOBAL_CONFIG.opt_recursive {
            while let Some(item) = queue.pop() {
                // check -- should deleted threads keep working?
                // exit/error on disconnected channel, which closes
                // at end of browse scope
                if hangup.load(Ordering::Relaxed) {
                    break;
                }

                if let Ok(mut items) = RecursiveSearch::enter_directory(
                    &item.path(),
                    &skim_tx,
                    &hangup,
                    &PathProvenance::IsPhantom,
                ) {
                    // disable behind deleted dirs with DepthOfOne,
                    // otherwise recurse and find all those deleted files
                    //
                    // don't propagate errors, errors we are most concerned about
                    // are transmission errors, which are handled elsewhere
                    queue.append(&mut items);
                }
            }
        }

        Ok(())
    }
}
