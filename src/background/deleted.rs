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
use crate::data::paths::BasicDirEntryInfo;
use crate::library::results::HttmResult;
use crate::GLOBAL_CONFIG;
use skim::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub struct DeletedSearch;

impl DeletedSearch {
    pub fn run_loop(
        deleted_dir: PathBuf,
        skim_tx: SkimItemSender,
        hangup: Arc<AtomicBool>,
    ) -> HttmResult<()> {
        let mut queue: Vec<BasicDirEntryInfo> = RecursiveSearch::enter_directory(
            &deleted_dir,
            &skim_tx,
            &hangup,
            &PathProvenance::IsPhantom,
        )?;

        if GLOBAL_CONFIG.opt_recursive {
            while let Some(item) = queue.pop() {
                // check -- should deleted threads keep working?
                // exit/error on disconnected channel, which closes
                // at end of browse scope
                if hangup.load(Ordering::Acquire) {
                    break;
                }

                if let Ok(mut res) = RecursiveSearch::enter_directory(
                    &item.path(),
                    &skim_tx,
                    &hangup,
                    &PathProvenance::IsPhantom,
                ) {
                    queue.append(&mut res);
                }
            }
        }

        Ok(())
    }
}
