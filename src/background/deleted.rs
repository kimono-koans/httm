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
    PathProvenance,
};
use crate::config::generate::DeletedMode;
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmResult;
use lscolors::Colorable;
use rayon::Scope;
use skim::prelude::*;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub struct DeletedSearch {
    opt_skim_tx: Option<SkimItemSender>,
    hangup: Arc<AtomicBool>,
}

impl DeletedSearch {
    fn new(opt_skim_tx: Option<SkimItemSender>, hangup: Arc<AtomicBool>) -> Self {
        Self {
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
            let _ = Self::new(opt_skim_tx, hangup).run_loop(&deleted_dir, None);
        })
    }
}

impl CommonSearch for DeletedSearch {
    fn hangup(&self) -> bool {
        self.hangup.load(Ordering::Relaxed)
    }

    fn entry_is_dir(&mut self, pseudo_entry: &BasicDirEntryInfo) -> bool {
        pseudo_entry.file_type().is_some_and(|ft| ft.is_dir())
    }

    fn run_loop(
        &mut self,
        deleted_dir: &Path,
        _opt_deleted_scope: Option<&Scope<'_>>,
    ) -> HttmResult<()> {
        // check to see whether we need to continue
        if self.hangup() {
            return Ok(());
        };

        let mut queue = Vec::new();

        self.enter_directory(deleted_dir, &mut queue)?;

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

    fn opt_sender(&self) -> Option<&SkimItemSender> {
        self.opt_skim_tx.as_ref()
    }

    fn path_provenance(&self) -> PathProvenance {
        PathProvenance::IsPhantom
    }
}
