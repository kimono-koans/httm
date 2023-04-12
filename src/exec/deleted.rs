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

use std::path::{Path, PathBuf};

use rayon::Scope;
use skim::prelude::*;

use crate::config::generate::DeletedMode;
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::exec::recursive::SharedRecursive;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{is_channel_closed, Never};
use crate::lookup::deleted::{DeletedFilesBundle, LastInTimeSet};
use crate::GLOBAL_CONFIG;

pub struct SpawnDeletedThread;

impl SpawnDeletedThread {
    // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
    pub fn exec(
        requested_dir: &Path,
        deleted_scope: &Scope,
        skim_tx: &SkimItemSender,
        hangup_rx: &Receiver<Never>,
    ) {
        // spawn_enumerate_deleted will send deleted files back to
        // the main thread for us
        let requested_dir_clone = requested_dir.to_path_buf();
        let skim_tx_clone = skim_tx.clone();
        let hangup_rx_clone = hangup_rx.clone();

        deleted_scope.spawn(move |_| {
            let _ = Self::enumerate(&requested_dir_clone, &skim_tx_clone, &hangup_rx_clone);
        });
    }

    // deleted file search for all modes
    fn enumerate(
        requested_dir: &Path,
        skim_tx: &SkimItemSender,
        hangup_rx: &Receiver<Never>,
    ) -> HttmResult<()> {
        // check -- should deleted threads keep working?
        // exit/error on disconnected channel, which closes
        // at end of browse scope
        if is_channel_closed(hangup_rx) {
            return Err(HttmError::new("Thread requested to quit.  Quitting.").into());
        }

        // obtain all unique deleted, unordered, unsorted, will need to fix
        let vec_deleted = DeletedFilesBundle::new(requested_dir);

        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            vec_deleted.into_inner().into_iter().partition(|entry| {
                // no need to traverse symlinks in deleted search
                SharedRecursive::is_entry_dir(entry)
            });

        SharedRecursive::combine_and_send_entries(
            vec_files,
            &vec_dirs,
            true,
            requested_dir,
            skim_tx,
        )?;

        // disable behind deleted dirs with DepthOfOne,
        // otherwise recurse and find all those deleted files
        //
        // don't propagate errors, errors we are most concerned about
        // are transmission errors, which are handled elsewhere
        if GLOBAL_CONFIG.opt_deleted_mode != Some(DeletedMode::DepthOfOne)
            && GLOBAL_CONFIG.opt_recursive
            && !vec_dirs.is_empty()
        {
            // get latest in time per our policy
            let path_set: Vec<PathData> = vec_dirs
                .into_iter()
                .map(|basic_info| PathData::from(&basic_info))
                .collect();

            let last_in_time_set = LastInTimeSet::new(&path_set);

            last_in_time_set.iter().try_for_each(|deleted_dir| {
                let requested_dir_clone = requested_dir.to_path_buf();

                Self::get_entries_behind_deleted_dir(
                    deleted_dir.as_path(),
                    &requested_dir_clone,
                    skim_tx,
                    hangup_rx,
                )
            })
        } else {
            Ok(())
        }
    }

    // searches for all files behind the dirs that have been deleted
    // recurses over all dir entries and creates pseudo live versions
    // for them all, policy is to use the latest snapshot version before
    // deletion
    fn get_entries_behind_deleted_dir(
        deleted_dir: &Path,
        requested_dir: &Path,
        skim_tx: &SkimItemSender,
        hangup_rx: &Receiver<Never>,
    ) -> HttmResult<()> {
        let mut queue = match &deleted_dir.file_name() {
            Some(dir_name) => {
                vec![RecurseBehindDeletedDir::new(
                    Path::new(dir_name),
                    deleted_dir.parent().unwrap_or_else(|| Path::new("/")),
                    requested_dir,
                    skim_tx,
                    hangup_rx,
                )?]
            }
            None => return Err(HttmError::new("Not a valid directory name!").into()),
        };

        while let Some(item) = queue.pop() {
            let res: HttmResult<Vec<RecurseBehindDeletedDir>> = item
                .vec_dirs
                .into_iter()
                .map(|basic_info| {
                    let dir_name = Path::new(basic_info.get_filename());
                    RecurseBehindDeletedDir::new(
                        dir_name,
                        &item.deleted_dir_on_snap,
                        &item.pseudo_live_dir,
                        skim_tx,
                        hangup_rx,
                    )
                })
                .collect();

            if let Ok(mut new_item) = res {
                queue.append(&mut new_item)
            }
        }

        Ok(())
    }
}

struct RecurseBehindDeletedDir {
    vec_dirs: Vec<BasicDirEntryInfo>,
    deleted_dir_on_snap: PathBuf,
    pseudo_live_dir: PathBuf,
}

impl RecurseBehindDeletedDir {
    fn new(
        dir_name: &Path,
        from_deleted_dir: &Path,
        from_requested_dir: &Path,
        skim_tx: &SkimItemSender,
        hangup_rx: &Receiver<Never>,
    ) -> HttmResult<RecurseBehindDeletedDir> {
        // check -- should deleted threads keep working?
        // exit/error on disconnected channel, which closes
        // at end of browse scope
        if is_channel_closed(hangup_rx) {
            return Err(HttmError::new("Thread requested to quit.  Quitting.").into());
        }

        // deleted_dir_on_snap is the path from the deleted dir on the snapshot
        // pseudo_live_dir is the path from the fake, deleted directory that once was
        let deleted_dir_on_snap = from_deleted_dir.to_path_buf().join(dir_name);
        let pseudo_live_dir = from_requested_dir.to_path_buf().join(dir_name);

        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            SharedRecursive::get_entries_partitioned(&deleted_dir_on_snap)?;

        SharedRecursive::combine_and_send_entries(
            vec_files,
            &vec_dirs,
            true,
            &pseudo_live_dir,
            skim_tx,
        )?;

        Ok(RecurseBehindDeletedDir {
            vec_dirs,
            deleted_dir_on_snap,
            pseudo_live_dir,
        })
    }
}
