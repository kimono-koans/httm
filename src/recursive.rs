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

    enumerate_directory(
        config_clone,
        &dummy_tx_item,
        &config.requested_dir.path_buf,
        out,
    )?;

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
    out: &mut Stdout,
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
                    let vec_deleted = get_deleted(&config, requested_dir)?;
                    if vec_deleted.is_empty() {
                        // Shows progress, while we are finding no deleted files
                        if config.opt_recursive {
                            eprint!(".");
                        }
                    } else {
                        // these are dummy placeholder values created from file on snapshots
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

                        let output_buf =
                            display_exec(&config, [vec_deleted, pseudo_live_versions])?;
                        // have to get a line break here, but shouldn't look unnatural
                        // print "." but don't print if in non-recursive mode
                        if config.opt_recursive {
                            eprintln!(".");
                        }
                        write!(out, "{}", output_buf)?;
                        out.flush()?;
                    }
                }
            }
        }
        ExecMode::Interactive => {
            // these are dummy placeholder values created from file on snapshots
            let get_pseudo_live_versions = |config: &Config,
                                            requested_dir: &Path|
             -> Result<
                Vec<PathBuf>,
                Box<dyn std::error::Error + Send + Sync + 'static>,
            > {
                let pseudo_live_versions: Vec<PathBuf> = get_deleted(config, requested_dir)?
                    .par_iter()
                    .map(|path| path.path_buf.file_name())
                    .flatten()
                    .map(|file_name| requested_dir.join(file_name))
                    .collect();
                Ok(pseudo_live_versions)
            };

            // combine dirs and files into a vec and sort to display
            let combined_vec: Vec<PathBuf> = match config.deleted_mode {
                DeletedMode::Only => get_pseudo_live_versions(&config, requested_dir)?,
                DeletedMode::Enabled => {
                    let pseudo_live_versions = get_pseudo_live_versions(&config, requested_dir)?;
                    vec![&vec_files, &vec_dirs, &pseudo_live_versions]
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
        vec_dirs
            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            .iter()
            .for_each(move |requested_dir| {
                let config_clone = config.clone();
                let _ = enumerate_directory(config_clone, tx_item, requested_dir, out);
            });
    }
    Ok(())
}
