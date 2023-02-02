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
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{path::Path, sync::Arc};

use rayon::Scope;
use skim::prelude::*;

use crate::config::generate::{Config, DeletedMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::exec::recursive::{
    combine_and_send_entries, get_entries_partitioned, recursive_is_entry_dir,
};
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{is_channel_closed, Never};
use crate::lookup::deleted::{DeletedFilesBundle, LastInTimeSet};

pub struct SpawnDeletedThread;

impl SpawnDeletedThread {
    // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
    pub fn exec(
        config: Arc<Config>,
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
            let _ = Self::enumerate(
                config,
                &requested_dir_clone,
                &skim_tx_clone,
                &hangup_rx_clone,
            );
        });
    }

    // deleted file search for all modes
    fn enumerate(
        config: Arc<Config>,
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
        let vec_deleted = DeletedFilesBundle::new(config.as_ref(), requested_dir);

        // combined entries will be sent or printed, but we need the vec_dirs to recurse
        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            vec_deleted.into_inner().into_iter().partition(|entry| {
                // no need to traverse symlinks in deleted search
                recursive_is_entry_dir(config.as_ref(), entry)
            });

        combine_and_send_entries(
            config.clone(),
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
        if config.opt_deleted_mode != Some(DeletedMode::DepthOfOne)
            && config.opt_recursive
            && !vec_dirs.is_empty()
        {
            // get latest in time per our policy
            let path_set: Vec<PathData> = vec_dirs
                .into_iter()
                .map(|basic_info| PathData::from(&basic_info))
                .collect();

            let last_in_time_set = LastInTimeSet::new(&config, &path_set);

            last_in_time_set.iter().try_for_each(|deleted_dir| {
                let config_clone = config.clone();
                let requested_dir_clone = requested_dir.to_path_buf();

                Self::get_entries_behind_deleted_dir(
                    config_clone,
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
        config: Arc<Config>,
        deleted_dir: &Path,
        requested_dir: &Path,
        skim_tx: &SkimItemSender,
        hangup_rx: &Receiver<Never>,
    ) -> HttmResult<()> {
        fn recurse_behind_deleted_dir(
            config: Arc<Config>,
            dir_name: &Path,
            from_deleted_dir: &Path,
            from_requested_dir: &Path,
            skim_tx: &SkimItemSender,
            hangup_rx: &Receiver<Never>,
        ) -> HttmResult<()> {
            // check -- should deleted threads keep working?
            // exit/error on disconnected channel, which closes
            // at end of browse scope
            if is_channel_closed(hangup_rx) {
                return Err(HttmError::new("Thread requested to quit.  Quitting.").into());
            }

            // deleted_dir_on_snap is the path from the deleted dir on the snapshot
            // pseudo_live_dir is the path from the fake, deleted directory that once was
            let deleted_dir_on_snap = &from_deleted_dir.to_path_buf().join(dir_name);
            let pseudo_live_dir = &from_requested_dir.to_path_buf().join(dir_name);

            let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
                get_entries_partitioned(config.as_ref(), deleted_dir_on_snap)?;

            combine_and_send_entries(
                config.clone(),
                vec_files,
                &vec_dirs,
                true,
                pseudo_live_dir,
                skim_tx,
            )?;

            // now recurse!
            // don't propagate errors, errors we are most concerned about
            // are transmission errors, which are handled elsewhere
            vec_dirs.into_iter().try_for_each(|basic_info| {
                recurse_behind_deleted_dir(
                    config.clone(),
                    Path::new(&basic_info.file_name),
                    deleted_dir_on_snap,
                    pseudo_live_dir,
                    skim_tx,
                    hangup_rx,
                )
            })
        }

        match &deleted_dir.file_name() {
            Some(dir_name) => recurse_behind_deleted_dir(
                config,
                Path::new(dir_name),
                deleted_dir.parent().unwrap_or_else(|| Path::new("/")),
                requested_dir,
                skim_tx,
                hangup_rx,
            ),
            None => Err(HttmError::new("Not a valid file name!").into()),
        }
    }
}
