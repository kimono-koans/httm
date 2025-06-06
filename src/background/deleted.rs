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

use crate::GLOBAL_CONFIG;
use crate::background::recursive::CommonSearch;
use crate::background::recursive::Entries;
use crate::background::recursive::PathProvenance;
use crate::background::recursive::enter_directory;
use crate::config::generate::DeletedMode;
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmError;
use crate::library::results::HttmResult;
use rayon::Scope;
use skim::prelude::*;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
//use std::thread::sleep;

pub struct DeletedSearch {
    deleted_dir: PathBuf,
    opt_skim_tx: Option<SkimItemSender>,
    hangup: Arc<AtomicBool>,
}

impl CommonSearch for &DeletedSearch {
    fn enter_directory(&self, requested_dir: &Path) -> HttmResult<Vec<BasicDirEntryInfo>> {
        enter_directory(self, requested_dir)
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
            let _ = Self::new(deleted_dir, opt_skim_tx.clone(), hangup.clone()).run_loop();
        })
    }

    fn run_loop(&self) -> HttmResult<()> {
        // check to see whether we need to continue
        self.hangup_check()?;

        // yield to other rayon work on this worker thread
        //self.timeout_loop()?;

        let mut queue: Vec<BasicDirEntryInfo> = self.enter_directory(&self.deleted_dir)?;

        if matches!(
            GLOBAL_CONFIG.opt_deleted_mode,
            Some(DeletedMode::DepthOfOne)
        ) {
            return Ok(());
        }

        if GLOBAL_CONFIG.opt_recursive {
            while let Some(item) = queue.pop() {
                // check to see whether we need to continue
                self.hangup_check()?;

                if let Ok(mut items) = self.enter_directory(&item.path()) {
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

    /*     fn timeout_loop(&self) -> HttmResult<()> {
        // yield to other rayon work on this worker thread

        let mut timeout = 1;

        loop {
            match rayon::yield_local() {
                Some(rayon::Yield::Executed) => {
                    self.hangup_check()?;

                    if timeout < 16 {
                        timeout *= 2
                    };

                    // wait timeout ms and then continue
                    sleep(std::time::Duration::from_millis(timeout));
                    continue;
                }
                Some(rayon::Yield::Idle) => break,
                None => unreachable!(
                    "None should be impossible as this loop should only ever execute on a Rayon thread."
                ),
            }
        }

        Ok(())
    } */

    fn hangup_check(&self) -> HttmResult<()> {
        if self.hangup() {
            return HttmError::new("Thread requested to hangup!").into();
        }

        Ok(())
    }
}
