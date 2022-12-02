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

use std::collections::VecDeque;
use std::{fs::read_dir, path::Path, sync::Arc};

use once_cell::unsync::OnceCell;
use rayon::{Scope, ThreadPool};
use skim::prelude::*;

use crate::config::generate::{Config, DeletedMode, ExecMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::exec::interactive::SelectionCandidate;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{
    httm_is_dir, is_channel_closed, nice_thread, print_output_buf, HttmIsDir, Never, PriorityType,
};
use crate::lookup::deleted::deleted_lookup_exec;
use crate::lookup::last_in_time::LastInTimeSet;
use crate::lookup::versions::versions_lookup_exec;
use crate::{BTRFS_SNAPPER_HIDDEN_DIRECTORY, ZFS_HIDDEN_DIRECTORY};

#[allow(unused_variables)]
pub fn display_recursive_wrapper(config: Arc<Config>) -> HttmResult<()> {
    // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
    let (dummy_skim_tx_item, _): (SkimItemSender, SkimItemReceiver) = unbounded();
    let (hangup_tx, hangup_rx): (Sender<Never>, Receiver<Never>) = bounded(0);
    let config_clone = config.clone();

    match &config.opt_requested_dir {
        Some(requested_dir) => {
            recursive_exec(
                config_clone,
                &requested_dir.path_buf,
                dummy_skim_tx_item,
                hangup_rx,
            );
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

pub fn recursive_exec(
    config: Arc<Config>,
    requested_dir: &Path,
    skim_tx_item: SkimItemSender,
    hangup_rx: Receiver<Never>,
) {
    let exec = |opt_deleted_scope: Option<&Scope>| {
        iterative_enumeration(
            config.clone(),
            requested_dir,
            opt_deleted_scope,
            &skim_tx_item,
            &hangup_rx,
        )
        .unwrap_or_else(|error| {
            eprintln!("Error: {}", error);
            std::process::exit(1)
        });
    };

    if config.deleted_mode.is_some() {
        // default stack size for rayon threads spawned to handle enumerate_deleted
        // here set at 1MB (the Linux default is 8MB) to avoid a stack overflow with the Rayon default
        const DEFAULT_STACK_SIZE: usize = 1_048_576;

        // build thread pool with a stack size large enough to avoid a stack overflow
        // this will be our one threadpool for directory enumeration ops
        let pool: ThreadPool = rayon::ThreadPoolBuilder::new()
            .stack_size(DEFAULT_STACK_SIZE)
            .build()
            .expect("Could not initialize rayon threadpool for recursive deleted search");

        pool.scope(|deleted_scope| exec(Some(deleted_scope)));
    } else {
        exec(None)
    }
}

fn iterative_enumeration(
    config: Arc<Config>,
    requested_dir: &Path,
    opt_deleted_scope: Option<&Scope>,
    skim_tx_item: &SkimItemSender,
    hangup_rx: &Receiver<Never>,
) -> HttmResult<()> {
    // runs once for non-recursive but also "primes the pump"
    // for recursive to have items available, also only place an
    // error can stop execution
    let mut queue: VecDeque<BasicDirEntryInfo> = enumerate_live(
        config.clone(),
        requested_dir,
        opt_deleted_scope,
        skim_tx_item,
        hangup_rx,
    )?
    .into();

    if config.opt_recursive {
        // condition kills iter when user has made a selection
        // pop_back makes this a LIFO queue which is supposedly better for caches
        while let Some(item) = queue.pop_back() {
            // no errors will be propagated in recursive mode
            // far too likely to run into a dir we don't have permissions to view
            if let Ok(vec_dirs) = enumerate_live(
                config.clone(),
                &item.path,
                opt_deleted_scope,
                skim_tx_item,
                hangup_rx,
            ) {
                queue.extend(vec_dirs.into_iter())
            }
        }
    }

    Ok(())
}

fn enumerate_live(
    config: Arc<Config>,
    requested_dir: &Path,
    opt_deleted_scope: Option<&Scope>,
    skim_tx_item: &SkimItemSender,
    hangup_rx: &Receiver<Never>,
) -> HttmResult<Vec<BasicDirEntryInfo>> {
    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
        get_entries_partitioned(config.as_ref(), requested_dir)?;

    combine_and_send_entries(
        config.clone(),
        vec_files,
        &vec_dirs,
        false,
        requested_dir,
        skim_tx_item,
    )?;

    if let Some(deleted_scope) = opt_deleted_scope {
        spawn_deleted(
            config,
            requested_dir,
            deleted_scope,
            skim_tx_item,
            hangup_rx,
        );
    }

    Ok(vec_dirs)
}

fn combine_and_send_entries(
    config: Arc<Config>,
    vec_files: Vec<BasicDirEntryInfo>,
    vec_dirs: &[BasicDirEntryInfo],
    is_phantom: bool,
    requested_dir: &Path,
    skim_tx_item: &SkimItemSender,
) -> HttmResult<()> {
    let mut combined = vec_files;
    combined.extend_from_slice(vec_dirs);

    let entries = if is_phantom {
        // deleted - phantom
        get_pseudo_live_versions(combined, requested_dir)
    } else {
        // live - not phantom
        match config.deleted_mode {
            Some(DeletedMode::Only) => return Ok(()),
            Some(DeletedMode::DepthOfOne | DeletedMode::Enabled) | None => {
                // never show live files is display recursive/deleted only file mode
                if matches!(config.exec_mode, ExecMode::DisplayRecursive(_)) {
                    return Ok(());
                } else {
                    combined
                }
            }
        }
    };

    display_or_transmit(config, entries, is_phantom, skim_tx_item)
}

// "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
fn spawn_deleted(
    config: Arc<Config>,
    requested_dir: &Path,
    deleted_scope: &Scope,
    skim_tx_item: &SkimItemSender,
    hangup_rx: &Receiver<Never>,
) {
    // spawn_enumerate_deleted will send deleted files back to
    // the main thread for us, so we can skip collecting deleted here
    // and return an empty vec
    let requested_dir_clone = requested_dir.to_path_buf();
    let skim_tx_item_clone = skim_tx_item.clone();
    let hangup_rx_clone = hangup_rx.clone();

    deleted_scope.spawn(move |_| {
            let _ = enumerate_deleted(
                config,
                &requested_dir_clone,
                &skim_tx_item_clone,
                &hangup_rx_clone,
            );
        });
}

fn get_entries_partitioned(
    config: &Config,
    requested_dir: &Path,
) -> HttmResult<(Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>)> {
    //separates entries into dirs and files
    let (vec_dirs, vec_files) = read_dir(&requested_dir)?
        .flatten()
        // checking file_type on dir entries is always preferable
        // as it is much faster than a metadata call on the path
        .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry))
        .filter(|entry| {
            if config.opt_no_filter {
                return true;
            } else if let Ok(file_type) = entry.get_filetype() {
                if file_type.is_dir() {
                    return !is_filter_dir(config, entry);
                }
            }
            true
        })
        .partition(|entry| recursive_is_entry_dir(config, entry));

    Ok((vec_dirs, vec_files))
}

fn recursive_is_entry_dir(config: &Config, entry: &BasicDirEntryInfo) -> bool {
    // must do is_dir() look up on file type as look up on path will traverse links!
    if config.opt_no_traverse {
        if let Ok(file_type) = entry.get_filetype() {
            return file_type.is_dir();
        }
    }
    httm_is_dir(entry)
}

fn is_filter_dir(config: &Config, entry: &BasicDirEntryInfo) -> bool {
    // FYI path is always a relative path, but no need to canonicalize as
    // partial eq for paths is comparison of components iter
    let path = entry.path.as_path();

    // never check the hidden snapshot directory for live files (duh)
    // didn't think this was possible until I saw a SMB share return
    // a .zfs dir entry
    if path.ends_with(ZFS_HIDDEN_DIRECTORY) || path.ends_with(BTRFS_SNAPPER_HIDDEN_DIRECTORY) {
        return true;
    }

    // is 1) a common snapshot path for btrfs, or 2) is a non-supported (non-ZFS, non-btrfs) dataset?

    // is a common btrfs snapshot dir?
    if let Some(common_snap_dir) = &config.dataset_collection.opt_common_snap_dir {
        if path == *common_snap_dir {
            return true;
        }
    }

    let user_requested_dir = config
        .opt_requested_dir
        .as_ref()
        .expect("opt_requested_dir must always be Some in any recursive mode")
        .path_buf
        .as_path();

    // check whether user requested this dir specifically, then we will show
    if path == user_requested_dir {
        false
    } else {
        // else: is a non-supported dataset?
        config.dataset_collection.filter_dirs.contains(path)
    }
}

// deleted file search for all modes
fn enumerate_deleted(
    config: Arc<Config>,
    requested_dir: &Path,
    skim_tx_item: &SkimItemSender,
    hangup_rx: &Receiver<Never>,
) -> HttmResult<()> {
    // check -- should deleted threads keep working?
    // exit/error on disconnected channel, which closes
    // at end of browse scope
    if is_channel_closed(hangup_rx) {
        return Err(HttmError::new("Thread requested to quit.  Quitting.").into());
    }

    // re-nice thread
    // use a lower priority to make room for interactive views/non-deleted enumeration
    if matches!(config.exec_mode, ExecMode::Interactive(_)) {
        // don't panic on failure setpriority failure
        let _ = nice_thread(PriorityType::Process, None, 2i32);
    }

    // obtain all unique deleted, unordered, unsorted, will need to fix
    let vec_deleted = deleted_lookup_exec(config.as_ref(), requested_dir);

    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
        vec_deleted.into_iter().partition(|entry| {
            // no need to traverse symlinks in deleted search
            recursive_is_entry_dir(config.as_ref(), entry)
        });

    combine_and_send_entries(
        config.clone(),
        vec_files,
        &vec_dirs,
        true,
        requested_dir,
        skim_tx_item,
    )?;

    // disable behind deleted dirs with DepthOfOne,
    // otherwise recurse and find all those deleted files
    //
    // don't propagate errors, errors we are most concerned about
    // are transmission errors, which are handled elsewhere
    if config.deleted_mode != Some(DeletedMode::DepthOfOne)
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

            get_entries_behind_deleted_dir(
                config_clone,
                deleted_dir.as_path(),
                &requested_dir_clone,
                skim_tx_item,
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
    skim_tx_item: &SkimItemSender,
    hangup_rx: &Receiver<Never>,
) -> HttmResult<()> {
    fn recurse_behind_deleted_dir(
        config: Arc<Config>,
        dir_name: &Path,
        from_deleted_dir: &Path,
        from_requested_dir: &Path,
        skim_tx_item: &SkimItemSender,
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
        let deleted_dir_on_snap = &from_deleted_dir.to_path_buf().join(&dir_name);
        let pseudo_live_dir = &from_requested_dir.to_path_buf().join(&dir_name);

        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            get_entries_partitioned(config.as_ref(), deleted_dir_on_snap)?;

        combine_and_send_entries(
            config.clone(),
            vec_files,
            &vec_dirs,
            true,
            pseudo_live_dir,
            skim_tx_item,
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
                skim_tx_item,
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
            skim_tx_item,
            hangup_rx,
        ),
        None => Err(HttmError::new("Not a valid file name!").into()),
    }
}

// this function creates dummy "live versions" values to match deleted files
// which have been found on snapshots, we return to the user "the path that
// once was" in their browse panel
fn get_pseudo_live_versions(
    entries: Vec<BasicDirEntryInfo>,
    pseudo_live_dir: &Path,
) -> Vec<BasicDirEntryInfo> {
    entries
        .into_iter()
        .map(|basic_info| BasicDirEntryInfo {
            path: pseudo_live_dir.join(&basic_info.file_name),
            file_name: basic_info.file_name,
            file_type: basic_info.file_type,
            modify_time: OnceCell::new(),
        })
        .collect()
}

fn display_or_transmit(
    config: Arc<Config>,
    entries: Vec<BasicDirEntryInfo>,
    is_phantom: bool,
    skim_tx_item: &SkimItemSender,
) -> HttmResult<()> {
    // send to the interactive view, or print directly, never return back
    match &config.exec_mode {
        ExecMode::Interactive(_) => {
            transmit_entries(config.clone(), entries, is_phantom, skim_tx_item)?
        }
        ExecMode::DisplayRecursive(progress_bar) => {
            if entries.is_empty() {
                progress_bar.tick();
            } else {
                print_display_recursive(config.as_ref(), entries)?;
                // keeps spinner from squashing last line of output
                eprintln!();
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}

fn transmit_entries(
    config: Arc<Config>,
    entries: Vec<BasicDirEntryInfo>,
    is_phantom: bool,
    skim_tx_item: &SkimItemSender,
) -> HttmResult<()> {
    // don't want a par_iter here because it will block and wait for all
    // results, instead of printing and recursing into the subsequent dirs
    entries
        .into_iter()
        .try_for_each(|basic_info| {
            skim_tx_item.try_send(Arc::new(SelectionCandidate::new(
                config.clone(),
                basic_info,
                is_phantom,
            )))
        })
        .map_err(std::convert::Into::into)
}

fn print_display_recursive(config: &Config, entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
    let pseudo_live_set: Vec<PathData> = entries
        .iter()
        .map(|basic_info| PathData::from(basic_info.path.as_path()))
        .collect();

    let map_live_to_snaps = versions_lookup_exec(config, &pseudo_live_set)?;

    let output_buf = map_live_to_snaps.display(config);

    print_output_buf(output_buf)
}
