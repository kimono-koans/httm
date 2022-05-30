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

use std::{ffi::OsStr, fs::read_dir, io::Write, path::Path, path::PathBuf, sync::Arc};

use rayon::{prelude::*, Scope};
use skim::prelude::*;

use crate::deleted::get_unique_deleted;
use crate::display::display_exec;
use crate::interactive::SelectionCandidate;
use crate::lookup::get_versions;
use crate::utility::httm_is_dir;
use crate::{
    BasicDirEntryInfo, Config, DeletedMode, ExecMode, HttmError, PathData, SnapPoint,
    ZFS_HIDDEN_DIRECTORY,
};

pub fn display_recursive_wrapper(
    config: &Config,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
    let (dummy_tx_item, _): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = Arc::new(config.clone());

    match &config.clone().requested_dir {
        Some(requested_dir) => {
            let requested_dir_clone = requested_dir.path_buf.clone();
            recursive_exec(config_clone, &dummy_tx_item, requested_dir_clone)?;
        }
        None => {
            return Err(HttmError::new(
                "requested_dir should never be None in Display Recursive mode",
            )
            .into())
        }
    }

    // flush and exit successfully upon ending recursive search
    if config.opt_recursive {
        let mut out = std::io::stdout();
        writeln!(out)?;
        out.flush()?;
    }

    std::process::exit(0)
}

pub fn recursive_exec(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build thread pool with a stack size large enough to avoid a stack overflow
    // 8MB is the default Linux thread stack size
    let thread_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .stack_size(8388608)
        .build()
        .unwrap();

    // pass this thread pools scope to enumerate_directory, and spawn threads from within this scope
    thread_pool.scope(|scope| {
        let _ = enumerate_live_versions(config, tx_item, requested_dir, scope);
    });
    Ok(())
}

fn enumerate_live_versions(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: PathBuf,
    scope: &Scope,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
        read_dir(&requested_dir)?
            .flatten()
            .par_bridge()
            // never check the hidden snapshot directory for live files (duh)
            // didn't think this was possible until I saw a SMB share return
            // a .zfs dir entry
            .filter(|dir_entry| {
                dir_entry.file_name().as_os_str() != OsStr::new(ZFS_HIDDEN_DIRECTORY)
            })
            // checking file_type on dir entries is always preferable
            // as it is much faster than a metadata call on the path
            .map(|dir_entry| BasicDirEntryInfo {
                file_name: dir_entry.file_name(),
                path: dir_entry.path(),
                file_type: dir_entry.file_type().ok(),
            })
            .partition(|entry| httm_is_dir(entry));

    let spawn_enumerate_deleted = || {
        let config_clone = config.clone();
        let requested_dir_clone = requested_dir.to_path_buf();
        let tx_item_clone = tx_item.clone();

        if config.exec_mode == ExecMode::Interactive {
            if let SnapPoint::Native(_) = config.snap_point {
                // "spawn" a lighter weight rayon/greenish thread for enumerate_deleted
                scope.spawn(move |_| {
                    let _ = enumerate_deleted(config_clone, &requested_dir_clone, &tx_item_clone);
                });
                return;
            }
        }
        // no join handles for these rayon threads, therefore we can't be certain when they
        // are all done executing, therefore we turn them off in the non-interactive modes
        let _ = enumerate_deleted(config_clone, &requested_dir_clone, &tx_item_clone);
    };

    match config.exec_mode {
        ExecMode::Display => unreachable!(),
        ExecMode::DisplayRecursive => {
            match config.deleted_mode {
                // display recursive in DeletedMode::Disabled may be
                // something to implement in the future but I'm not sure
                // it really makes sense, as it's only really good for a
                // small subset of files
                DeletedMode::Disabled => unreachable!(),
                // for all other non-disabled DeletedModes
                DeletedMode::DepthOfOne | DeletedMode::Enabled | DeletedMode::Only => {
                    spawn_enumerate_deleted();
                }
            }
        }
        ExecMode::Interactive => {
            let combined_vec = || {
                let mut combined = vec_files;
                combined.extend(vec_dirs.clone());
                combined
            };

            // combine dirs and files into a vec and sort to display
            let entries: Vec<BasicDirEntryInfo> = match config.deleted_mode {
                DeletedMode::Only => {
                    // spawn_enumerate_deleted will send deleted files back to
                    // the main thread for us, so we can skip collecting deleted here
                    // and return an empty vec
                    spawn_enumerate_deleted();
                    Vec::new()
                }
                DeletedMode::DepthOfOne | DeletedMode::Enabled => {
                    spawn_enumerate_deleted();
                    combined_vec()
                }
                DeletedMode::Disabled => combined_vec(),
            };

            // is_phantom is false because these are known live entries
            send_entries(config.clone(), entries, false, tx_item)?;
        }
    }

    // now recurse into those dirs, if requested
    if config.opt_recursive {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        vec_dirs.into_iter().for_each(move |requested_dir| {
            let _ = enumerate_live_versions(config.clone(), tx_item, requested_dir.path, scope);
        });
    }

    Ok(())
}

// deleted file search for all modes
fn enumerate_deleted(
    config: Arc<Config>,
    requested_dir: &Path,
    tx_item: &SkimItemSender,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // obtain all unique deleted, policy is one version for each file, latest in time
    let deleted = get_unique_deleted(&config, requested_dir)?;

    // combined entries will be sent or printed, but we need the vec_dirs to recurse
    let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) = deleted
        .into_iter()
        .partition(|basic_dir_entry_info| httm_is_dir(basic_dir_entry_info));

    // disable behind deleted dirs with DepthOfOne,
    // otherwise recurse and find all those deleted files
    if config.deleted_mode != DeletedMode::DepthOfOne {
        let _ = &vec_dirs
            .iter()
            .map(|basic_dir_entry_info| basic_dir_entry_info.path.to_owned())
            .for_each(|deleted_dir| {
                let config_clone = config.clone();
                let requested_dir_clone = requested_dir.to_path_buf();
                let tx_item_clone = tx_item.clone();

                let _ = behind_deleted_dir(
                    config_clone,
                    &tx_item_clone,
                    &deleted_dir,
                    &requested_dir_clone,
                );
            });
    }

    // partition above is needed as vec_files will be used later
    // to determine dirs to recurse, here, we recombine to obtain
    // pseudo live versions of deleted files, files that once were
    let mut entries = vec_files;
    entries.extend(vec_dirs);
    let pseudo_live_versions: Vec<BasicDirEntryInfo> =
        get_pseudo_live_versions(entries, requested_dir);

    match config.exec_mode {
        // know this is_phantom because we know it is deleted
        ExecMode::Interactive => send_entries(config, pseudo_live_versions, true, tx_item)?,
        ExecMode::DisplayRecursive => {
            if !pseudo_live_versions.is_empty() {
                if config.opt_recursive {
                    eprintln!();
                }
                print_deleted_recursive(config, pseudo_live_versions)?
            } else if config.opt_recursive {
                eprint!(".");
            }
        }
        _ => unreachable!(),
    }

    Ok(())
}

// searches for all files behind the dirs that have been deleted
// recurses over all dir entries and creates pseudo live versions
// for them all, policy is to use the latest snapshot version before
// deletion
fn behind_deleted_dir(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    deleted_dir: &Path,
    requested_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    fn recurse_behind_deleted_dir(
        config: Arc<Config>,
        tx_item: &SkimItemSender,
        dir_name: &Path,
        from_deleted_dir: &Path,
        from_requested_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        // deleted_dir_on_snap is the path from the deleted dir on the snapshot
        // pseudo_live_dir is the path from the fake, deleted directory that once was
        let deleted_dir_on_snap = &from_deleted_dir.to_path_buf().join(&dir_name);
        let pseudo_live_dir = &from_requested_dir.to_path_buf().join(&dir_name);

        let (vec_dirs, vec_files): (Vec<BasicDirEntryInfo>, Vec<BasicDirEntryInfo>) =
            read_dir(&deleted_dir_on_snap)?
                .flatten()
                .map(|dir_entry| BasicDirEntryInfo {
                    file_name: dir_entry.file_name(),
                    path: dir_entry.path(),
                    file_type: dir_entry.file_type().ok(),
                })
                .partition(|entry| httm_is_dir(entry));

        // partition above is needed as vec_files will be used later
        // to determine dirs to recurse, here, we recombine to obtain
        // pseudo live versions of deleted files, files that once were
        let mut entries = vec_files;
        entries.extend(vec_dirs.clone());
        let pseudo_live_versions: Vec<BasicDirEntryInfo> =
            get_pseudo_live_versions(entries, pseudo_live_dir);

        // send to the interactive view, or print directly, never return back
        match config.exec_mode {
            ExecMode::Interactive => {
                // know this is_phantom because we know it is deleted
                send_entries(config.clone(), pseudo_live_versions, true, tx_item)?
            }
            ExecMode::DisplayRecursive => {
                if !pseudo_live_versions.is_empty() {
                    if config.opt_recursive {
                        eprintln!();
                    }
                    print_deleted_recursive(config.clone(), pseudo_live_versions)?
                } else if config.opt_recursive {
                    eprint!(".");
                }
            }
            _ => unreachable!(),
        }

        vec_dirs.into_iter().for_each(|basic_dir_entry_info| {
            let _ = recurse_behind_deleted_dir(
                config.clone(),
                tx_item,
                Path::new(&basic_dir_entry_info.file_name),
                deleted_dir_on_snap,
                pseudo_live_dir,
            );
        });

        Ok(())
    }

    match &deleted_dir.file_name() {
        Some(dir_name) => recurse_behind_deleted_dir(
            config,
            tx_item,
            Path::new(dir_name),
            deleted_dir.parent().unwrap_or_else(|| Path::new("/")),
            requested_dir,
        )?,
        None => return Err(HttmError::new("Not a valid file!").into()),
    }

    Ok(())
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
        })
        .collect()
}

fn send_entries(
    config: Arc<Config>,
    entries: Vec<BasicDirEntryInfo>,
    is_phantom: bool,
    tx_item: &SkimItemSender,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // don't want a par_iter here because it will block and wait for all
    // results, instead of printing and recursing into the subsequent dirs
    entries.into_iter().for_each(|basic_dir_entry_info| {
        let _ = tx_item.send(Arc::new(SelectionCandidate::new(
            config.clone(),
            basic_dir_entry_info.file_name,
            basic_dir_entry_info.path,
            basic_dir_entry_info.file_type,
            is_phantom,
        )));
    });

    Ok(())
}

fn print_deleted_recursive(
    config: Arc<Config>,
    path_buf_set: Vec<BasicDirEntryInfo>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pseudo_live_set: Vec<PathData> = path_buf_set
        .iter()
        .map(|basic_dir_entry_info| PathData::from(basic_dir_entry_info.path.as_path()))
        .collect();

    let snaps_and_live_set = get_versions(&config, &pseudo_live_set)?;

    let mut out = std::io::stdout();
    let output_buf = display_exec(&config, snaps_and_live_set)?;
    let _ = write!(out, "{}", output_buf);
    out.flush()?;

    Ok(())
}
