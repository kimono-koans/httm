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

use crate::deleted::get_deleted;
use crate::display::display_exec;
use crate::interactive::SelectionCandidate;
use crate::utility::httm_is_dir;
use crate::{Config, DeletedMode, ExecMode, PathData};

use rayon::{iter::Either, prelude::*};
use skim::prelude::*;
use std::{
    fs::read_dir,
    io::{Stdout, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn display_recursive_exec(
    config: &Config,
    out: &mut Stdout,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // won't be sending anything anywhere, this just allows us to reuse enumerate_directory
    let (dummy_tx_item, _): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = Arc::new(config.clone());

    enumerate_directory(config_clone, &dummy_tx_item, &config.requested_dir.path_buf)?;

    // flush and exit successfully upon ending recursive search
    if config.opt_recursive {
        println!();
        out.flush()?;
    }
    std::process::exit(0)
}

pub fn enumerate_directory(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let (vec_dirs, vec_files): (Vec<PathBuf>, Vec<PathBuf>) = read_dir(&requested_dir)?
        .flatten()
        .par_bridge()
        // checking file_type on dir entries is always preferable
        // as it is much faster than a metadata call on the path
        .partition_map(|dir_entry| {
            let path = dir_entry.path();
            if httm_is_dir(&dir_entry) {
                Either::Left(path)
            } else {
                Either::Right(path)
            }
        });

    // need something to hold our threads that we need to wait to have complete,
    // also very helpful in the case we don't don't needs threads as it can be empty
    let mut join_handles = Vec::new();

    match config.exec_mode {
        ExecMode::Display => unreachable!(),
        ExecMode::DisplayRecursive => {
            match config.deleted_mode {
                // display recursive in DeletedMode::Disabled may be
                // something to implement in the future but I'm not sure
                // it really makes sense, as it's only really good for a
                // small subset of files
                DeletedMode::Disabled => unreachable!(),
                // for all other non-disabled DeletedModes we display
                // all deleted files in ExecMode::DisplayRecursive
                DeletedMode::Enabled | DeletedMode::Only => {
                    let config_clone = config.clone();
                    let requested_dir_clone = requested_dir.to_owned();

                    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
                    let print_recursive_handle = std::thread::spawn(move || {
                        let _ = print_deleted_recursive(config_clone, &requested_dir_clone);
                    });
                    join_handles.push(print_recursive_handle);
                }
            }
        }
        ExecMode::Interactive => {
            let mut spawn_enumerate_deleted = || {
                let config_clone = config.clone();
                let requested_dir_clone = requested_dir.to_owned();
                let tx_item_clone = tx_item.clone();

                // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
                let pseudo_live_handle = std::thread::spawn(move || {
                    let _ = get_pseudo_live_versions(
                        config_clone,
                        &requested_dir_clone,
                        &tx_item_clone,
                    );
                });
                join_handles.push(pseudo_live_handle);
            };

            // combine dirs and files into a vec and sort to display
            let combined_vec: Vec<PathBuf> = match config.deleted_mode {
                DeletedMode::Only => {
                    spawn_enumerate_deleted();
                    // spawn_enumerate_deleted will send deleted files back to
                    // the main thread for us, so we can skip collecting deleted here
                    // and return an empty vec
                    Vec::new()
                }
                DeletedMode::Enabled => {
                    spawn_enumerate_deleted();
                    // spawn_enumerate_deleted will send deleted files back to
                    // the main thread for us, so we can skip collecting a
                    // vec_deleted here
                    vec![&vec_files, &vec_dirs]
                        .into_par_iter()
                        .flatten()
                        .cloned()
                        .collect()
                }
                DeletedMode::Disabled => vec![&vec_files, &vec_dirs]
                    .into_par_iter()
                    .flatten()
                    .cloned()
                    .collect(),
            };

            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            combined_vec.iter().for_each(|path| {
                let _ = tx_item.send(Arc::new(SelectionCandidate::new(
                    config.clone(),
                    path.to_path_buf(),
                )));
            });
        }
    }

    // now recurse into those dirs, if requested
    if config.opt_recursive {
        // don't want a par_iter here because it will block and wait for all
        // results, instead of printing and recursing into the subsequent dirs
        vec_dirs.iter().for_each(|requested_dir| {
            let _ = enumerate_directory(config.clone(), tx_item, requested_dir);
        });
    }

    // here we make sure to wait until all child threads have exited before returning
    if !join_handles.is_empty() {
        let _ = join_handles
            .into_iter()
            .try_for_each(|handle| handle.join());
    }

    Ok(())
}

// these are dummy "live versions" values generated to match deleted files
// which have been found on snapshots, we return to the user "the path that
// once was" in their browse panel
//
// why another fn? so we can spawn another thread and not block a finding deleted files
// generally takes a long time.
fn get_pseudo_live_versions(
    config: Arc<Config>,
    requested_dir: &Path,
    tx_item: &SkimItemSender,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pseudo_live_versions: Vec<PathBuf> = get_deleted(&config, requested_dir)?
        .par_iter()
        .map(|path| path.path_buf.file_name())
        .flatten()
        .map(|file_name| requested_dir.join(file_name))
        .collect();

    pseudo_live_versions.iter().for_each(|path| {
        let _ = tx_item.send(Arc::new(SelectionCandidate::new(
            config.clone(),
            path.to_path_buf(),
        )));
    });

    Ok(())
}

fn print_deleted_recursive(
    config: Arc<Config>,
    requested_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let vec_deleted = get_deleted(&config, requested_dir)?;
    if vec_deleted.is_empty() {
        // Shows progress, while we are finding no deleted files
        if config.opt_recursive {
            eprint!(".");
        }
    } else {
        // these are dummy "live versions" values generated to match deleted files
        // which have been found on snapshots, to combine with the delete files
        // on snapshots to make a snaps and live set
        let pseudo_live_versions: Vec<PathData> = if !config.opt_no_live_vers {
            let mut res: Vec<PathData> = vec_deleted
                .par_iter()
                .map(|path| path.path_buf.file_name())
                .flatten()
                .map(|file_name| requested_dir.join(file_name))
                .map(|path| PathData::from(path.as_path()))
                .collect();
            res.par_sort_unstable_by_key(|pathdata| pathdata.path_buf.clone());
            res
        } else {
            Vec::new()
        };

        let output_buf = display_exec(&config, [vec_deleted, pseudo_live_versions])?;
        // have to get a line break here, but shouldn't look unnatural
        // print "." but don't print if in non-recursive mode
        if config.opt_recursive {
            eprintln!(".");
        }
        let mut out = std::io::stdout();
        writeln!(out, "{}", output_buf)?;
        out.flush()?;
    }
    Ok(())
}
