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
use std::{
    fs::{read_dir, DirEntry},
    path::{Path, PathBuf},
    sync::Arc,
};

use indicatif::ProgressBar;
use once_cell::unsync::OnceCell;
use rayon::{prelude::*, Scope, ThreadPool};
use skim::prelude::*;

use crate::display::display_exec;
use crate::interactive::SelectionCandidate;
use crate::lookup_deleted::deleted_lookup_exec;
use crate::lookup_versions::versions_lookup_exec;
use crate::utility::{httm_is_dir, print_output_buf, BasicDirEntryInfo, HttmError, PathData};
use crate::{
    Config, DeletedMode, ExecMode, HttmResult, BTRFS_SNAPPER_HIDDEN_DIRECTORY, ZFS_HIDDEN_DIRECTORY,
};

pub fn display_recursive_wrapper(config: Arc<Config>) -> HttmResult<()> {
    // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
    let (dummy_tx_item, _): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = config.clone();

    match &config.requested_dir {
        Some(requested_dir) => {
            recursive_exec(config_clone, &dummy_tx_item, &requested_dir.path_buf)?;
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
    tx_item: &SkimItemSender,
    requested_dir: &Path,
) -> HttmResult<()> {
    // default stack size for rayon threads spawned to handle enumerate_deleted
    // here set at 1MB (the Linux default is 8MB) to avoid a stack overflow with the Rayon default
    const DEFAULT_STACK_SIZE: usize = 1048576;
    const DEFAULT_NUM_THREADS: usize = 2;

    // build thread pool with a stack size large enough to avoid a stack overflow
    // this will be our one threadpool for directory enumeration ops
    lazy_static! {
        static ref THREAD_POOL: ThreadPool = rayon::ThreadPoolBuilder::new()
            .stack_size(DEFAULT_STACK_SIZE)
            // limit # of threads available for deleted search
            .num_threads(DEFAULT_NUM_THREADS)
            .build()
            .expect("Could not initialize rayon threadpool for recursive search");
    }

    THREAD_POOL.in_place_scope(|deleted_scope| {
        iterate_over_live_directories(config.clone(), tx_item, requested_dir, deleted_scope)
            .unwrap_or_else(|error| {
                eprintln!("Error: {}", error);
                std::process::exit(1)
            })
    });

    Ok(())
}

// and iterative approach seems to be *way faster* and less CPU intensive vs. recursive with Rust
fn iterate_over_live_directories(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
    deleted_scope: &Scope,
) -> HttmResult<()> {
    let initial_vec_dirs =
        enter_live_directory(config.clone(), tx_item, requested_dir, deleted_scope)?;

    if config.opt_recursive {
        let mut recursive_vec_dirs = initial_vec_dirs;

        while !recursive_vec_dirs.is_empty() {
            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            recursive_vec_dirs = recursive_vec_dirs
                .into_iter()
                // flatten errors here (e.g. just not worth it to exit
                // on bad permissions error for a recursive directory) so
                // should fail on /root but on stop exec on /
                .flat_map(|requested_dir| {
                    enter_live_directory(
                        config.clone(),
                        tx_item,
                        &requested_dir.path,
                        deleted_scope,
                    )
                })
                .flatten()
                .collect();
        }
    }

    Ok(())
}

fn enter_live_directory(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
    deleted_scope: &Scope,
) -> HttmResult<Vec<BasicDirEntryInfo>> {
    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
        get_entries_partitioned(config.as_ref(), requested_dir)?;

    // check exec mode and deleted mode, we do something different for each
    match config.exec_mode {
        ExecMode::Display | ExecMode::SnapFileMount | ExecMode::MountsForFiles => unreachable!(),
        ExecMode::DisplayRecursive => {
            match config.deleted_mode {
                // display recursive in DeletedMode::Disabled may be
                // something to implement in the future but I'm not sure
                // it really makes sense, as it's only really good for a
                // small subset of files
                DeletedMode::Disabled => unreachable!(),
                // for all other non-disabled DeletedModes
                DeletedMode::DepthOfOne | DeletedMode::Enabled | DeletedMode::Only => {
                    // scope guarantees that all threads finish before we exit
                    spawn_enumerate_deleted(config.clone(), requested_dir, tx_item, deleted_scope);
                }
            }
        }
        ExecMode::Interactive | ExecMode::LastSnap(_) => {
            // recombine dirs and files into a vec
            let combined_vec = || {
                let mut combined = vec_files;
                combined.extend_from_slice(&vec_dirs);
                combined
            };

            let entries: Vec<BasicDirEntryInfo> = match config.deleted_mode {
                DeletedMode::Only => {
                    // spawn_enumerate_deleted will send deleted files back to
                    // the main thread for us, so we can skip collecting deleted here
                    // and return an empty vec
                    spawn_enumerate_deleted(config.clone(), requested_dir, tx_item, deleted_scope);
                    Vec::new()
                }
                DeletedMode::DepthOfOne | DeletedMode::Enabled => {
                    // DepthOfOne will be handled inside enumerate_deleted
                    spawn_enumerate_deleted(config.clone(), requested_dir, tx_item, deleted_scope);
                    combined_vec()
                }
                DeletedMode::Disabled => combined_vec(),
            };

            // is_phantom is false because these are known live entries
            display_or_transmit(config.clone(), entries, false, tx_item)?;
        }
    }

    Ok(vec_dirs)
}

fn get_entries_partitioned(
    config: &Config,
    requested_dir: &Path,
) -> HttmResult<(Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>)> {
    //separates entries into dirs and files
    let (vec_dirs, vec_files) = read_dir(&requested_dir)?
        .flatten()
        .par_bridge()
        // checking file_type on dir entries is always preferable
        // as it is much faster than a metadata call on the path
        .filter(|dir_entry| {
            if config.opt_no_filter {
                true
            } else {
                !is_filter_dir(config, dir_entry)
            }
        })
        .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry))
        .partition(|entry| httm_is_dir(entry));

    Ok((vec_dirs, vec_files))
}

fn is_filter_dir(config: &Config, dir_entry: &DirEntry) -> bool {
    // FYI path is always a relative path, but no need to canonicalize as
    // partial eq for paths is comparison of components iter
    let path = dir_entry.path();

    // never check the hidden snapshot directory for live files (duh)
    // didn't think this was possible until I saw a SMB share return
    // a .zfs dir entry
    if path.ends_with(ZFS_HIDDEN_DIRECTORY) || path.ends_with(BTRFS_SNAPPER_HIDDEN_DIRECTORY) {
        return true;
    }

    // is a common path for btrfs or is a non-supported dataset?

    // is a common btrfs snapshot dir?
    if let Some(common_snap_dir) = &config.dataset_collection.opt_common_snap_dir {
        if path == *common_snap_dir {
            return true;
        }
    }

    let interactive_requested_dir = config
        .requested_dir
        .as_ref()
        .expect("interactive_requested_dir must always be Some in any recursive mode")
        .path_buf
        .as_path();

    // check whether user requested this dir specifically, then we will show
    if path == interactive_requested_dir {
        false
    // else: is a non-supported dataset?
    } else {
        config
            .dataset_collection
            .vec_of_filter_dirs
            .par_iter()
            .any(|filter_dir| path == *filter_dir)
    }
}

// "spawn" a lighter weight rayon/greenish thread for enumerate_deleted, if needed
fn spawn_enumerate_deleted(
    config: Arc<Config>,
    requested_dir: &Path,
    tx_item: &SkimItemSender,
    deleted_scope: &Scope,
) {
    // clone items because new thread needs ownership
    let requested_dir_clone = requested_dir.to_path_buf();
    let tx_item_clone = tx_item.clone();

    deleted_scope.spawn(move |_| {
        let _ =
            iterate_over_deleted_directory(config.clone(), &requested_dir_clone, &tx_item_clone);
    });
}

// and iterative approach seems to be *way faster* and less CPU intensive vs. recursive with Rust
fn iterate_over_deleted_directory(
    config: Arc<Config>,
    requested_dir: &Path,
    tx_item: &SkimItemSender,
) -> HttmResult<()> {
    let initial_vec_dirs = enter_deleted_directory(config.clone(), requested_dir, tx_item)?;

    if config.deleted_mode != DeletedMode::DepthOfOne && config.opt_recursive {
        let mut recurse_dirs: Vec<DirsBehindDeletedDir> = initial_vec_dirs
            .into_iter()
            .flat_map(|basic_dir_entry_info| {
                recurse_behind_deleted_dir(
                    config.clone(),
                    &tx_item.clone(),
                    Path::new(&basic_dir_entry_info.file_name),
                    basic_dir_entry_info
                        .path
                        .parent()
                        .unwrap_or_else(|| Path::new("/")),
                    requested_dir,
                )
            })
            .collect();

        while !recurse_dirs.is_empty() {
            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            recurse_dirs = recurse_dirs
                .into_iter()
                // flatten errors here (e.g. just not worth it to exit
                // on bad permissions error for a recursive directory) so
                // should fail on /root but on stop exec on /
                .flat_map(|dirs_behind_deleted_dir| {
                    dirs_behind_deleted_dir
                        .vec_dirs
                        .into_iter()
                        .flat_map(|basic_dir_entry_info| {
                            recurse_behind_deleted_dir(
                                config.clone(),
                                tx_item,
                                &PathBuf::from(&basic_dir_entry_info.file_name),
                                &dirs_behind_deleted_dir.deleted_dir_on_snap,
                                &dirs_behind_deleted_dir.pseudo_live_dir,
                            )
                        })
                        .collect::<Vec<DirsBehindDeletedDir>>()
                })
                .collect();
        }
    }

    Ok(())
}

// deleted file search for all modes
fn enter_deleted_directory(
    config: Arc<Config>,
    requested_dir: &Path,
    tx_item: &SkimItemSender,
) -> HttmResult<Vec<BasicDirEntryInfo>> {
    // obtain all unique deleted, policy is one version for each file, latest in time
    let deleted = deleted_lookup_exec(&config, requested_dir)?;

    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) = deleted
        .into_iter()
        .partition(|basic_dir_entry_info| httm_is_dir(basic_dir_entry_info));

    // partition above is needed as vec_files will be used later
    // to determine dirs to recurse, here, we recombine to obtain
    // pseudo live versions of deleted files, files that once were
    let mut combined_entries = vec_files;
    // recombine our directories and files
    combined_entries.extend_from_slice(&vec_dirs);
    let pseudo_live_versions: Vec<BasicDirEntryInfo> =
        get_pseudo_live_versions(combined_entries, requested_dir);

    // know this is_phantom because we know it is deleted
    display_or_transmit(config, pseudo_live_versions, true, tx_item)?;

    Ok(vec_dirs)
}

struct DirsBehindDeletedDir {
    vec_dirs: Vec<BasicDirEntryInfo>,
    deleted_dir_on_snap: PathBuf,
    pseudo_live_dir: PathBuf,
}

// searches for all files behind the dirs that have been deleted
// recurses over all dir entries and creates pseudo live versions
// for them all, policy is to use the latest snapshot version before
// deletion
fn recurse_behind_deleted_dir(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    dir_name: &Path,
    from_deleted_dir: &Path,
    from_requested_dir: &Path,
) -> HttmResult<DirsBehindDeletedDir> {
    // deleted_dir_on_snap is the path from the deleted dir on the snapshot
    // pseudo_live_dir is the path from the fake, deleted directory that once was
    let deleted_dir_on_snap = from_deleted_dir.to_path_buf().join(&dir_name);
    let pseudo_live_dir = from_requested_dir.to_path_buf().join(&dir_name);

    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
        get_entries_partitioned(config.as_ref(), &deleted_dir_on_snap)?;

    // partition above is needed as vec_files will be used later
    // to determine dirs to recurse, here, we recombine to obtain
    // pseudo live versions of deleted files, files that once were
    let mut combined_entries = vec_files;
    // recombine our directories and files
    combined_entries.extend_from_slice(&vec_dirs);
    let pseudo_live_versions: Vec<BasicDirEntryInfo> =
        get_pseudo_live_versions(combined_entries, &pseudo_live_dir);

    // know this is_phantom because we know it is deleted
    display_or_transmit(config, pseudo_live_versions, true, tx_item)?;

    Ok(DirsBehindDeletedDir {
        vec_dirs,
        deleted_dir_on_snap,
        pseudo_live_dir,
    })
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
        .map(|basic_dir_entry_info| BasicDirEntryInfo {
            path: pseudo_live_dir.join(&basic_dir_entry_info.file_name),
            file_name: basic_dir_entry_info.file_name,
            file_type: basic_dir_entry_info.file_type,
            modify_time: OnceCell::new(),
        })
        .collect()
}

fn display_or_transmit(
    config: Arc<Config>,
    entries: Vec<BasicDirEntryInfo>,
    is_phantom: bool,
    tx_item: &SkimItemSender,
) -> HttmResult<()> {
    // send to the interactive view, or print directly, never return back
    match config.exec_mode {
        ExecMode::Interactive | ExecMode::LastSnap(_) => {
            transmit_entries(config.clone(), entries, is_phantom, tx_item)?
        }
        ExecMode::DisplayRecursive => {
            // passing a progress bar through multiple functions is a pain, and since we only need a global,
            // here we just create a static progress bar for Display Recursive mode
            lazy_static! {
                static ref PROGRESS_BAR: ProgressBar = indicatif::ProgressBar::new_spinner();
            }

            if entries.is_empty() {
                PROGRESS_BAR.tick();
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
    tx_item: &SkimItemSender,
) -> HttmResult<()> {
    // don't want a par_iter here because it will block and wait for all
    // results, instead of printing and recursing into the subsequent dirs
    entries.into_iter().for_each(|basic_dir_entry_info| {
        let _ = tx_item.send(Arc::new(SelectionCandidate::new(
            config.clone(),
            basic_dir_entry_info,
            is_phantom,
        )));
    });

    Ok(())
}

fn print_display_recursive(config: &Config, entries: Vec<BasicDirEntryInfo>) -> HttmResult<()> {
    let pseudo_live_set: Vec<PathData> = entries
        .iter()
        .map(|basic_dir_entry_info| PathData::from(basic_dir_entry_info.path.as_path()))
        .collect();

    let snaps_and_live_set = versions_lookup_exec(config, &pseudo_live_set)?;

    let output_buf = display_exec(config, &snaps_and_live_set)?;

    print_output_buf(output_buf)?;

    Ok(())
}
