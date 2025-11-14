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

//use std::thread::sleep;
use crate::GLOBAL_CONFIG;
use crate::background::recursive::{
    CommonSearch,
    Entries,
    PathProvenance,
    enter_directory,
};
use crate::config::generate::DeletedMode;
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmResult;
use lscolors::Colorable;
use rayon::Scope;
use skim::prelude::*;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub struct DeletedSearch {
    deleted_dir: PathBuf,
    opt_skim_tx: Option<SkimItemSender>,
    hangup: Arc<AtomicBool>,
}

impl DeletedSearch {
    fn new(
        deleted_dir: PathBuf,
        opt_skim_tx: Option<SkimItemSender>,
        hangup: Arc<AtomicBool>,
    ) -> Self {
        Self {
            deleted_dir,
            opt_skim_tx,
            hangup,
        }
    }

    // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
    pub fn spawn(
        requested_dir: &Path,
        deleted_scope: &Scope,
        opt_skim_tx: Option<SkimItemSender>,
        hangup: Arc<AtomicBool>,
    ) {
        let deleted_dir = requested_dir.to_path_buf();

        deleted_scope.spawn(move |_| {
            let _ = Self::new(deleted_dir, opt_skim_tx.clone(), hangup.clone()).run_loop(None);
        })
    }
}

impl CommonSearch for DeletedSearch {
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

    fn into_entries<'a>(&'a self, requested_dir: &'a Path) -> Entries<'a> {
        Entries::new(
            requested_dir,
            &PathProvenance::IsPhantom,
            self.opt_skim_tx.as_ref(),
        )
    }

    fn entry_is_dir(&self, pseudo_entry: &BasicDirEntryInfo) -> bool {
        pseudo_entry.file_type().is_some_and(|ft| ft.is_dir())
    }

    fn run_loop(&self, _opt_deleted_scope: Option<&Scope<'_>>) -> HttmResult<()> {
        // check to see whether we need to continue
        if self.hangup() {
            return Ok(());
        };

        let mut queue = Vec::new();

        self.enter_directory(&self.deleted_dir, &mut queue)?;

        if matches!(
            GLOBAL_CONFIG.opt_deleted_mode,
            Some(DeletedMode::DepthOfOne)
        ) {
            return Ok(());
        }

        if GLOBAL_CONFIG.opt_recursive {
            while let Some(item) = queue.pop() {
                // check to see whether we need to continue
                if self.hangup() {
                    return Ok(());
                }

                let _ = self.enter_directory(&item.path(), &mut queue);
            }
        }

        Ok(())
    }
}
