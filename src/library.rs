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
use crate::{Config, DeletedMode, ExecMode, PathData};

use lscolors::{LsColors, Style};
use rayon::iter::Either;
use rayon::prelude::*;
use skim::prelude::*;
use std::{
    io::Stdout,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    stdin.read_line(&mut buffer)?;

    let broken_string: Vec<String> = buffer
        .split_ascii_whitespace()
        .into_iter()
        .map(|i| i.to_owned())
        .collect();

    Ok(broken_string)
}

pub fn enumerate_directory(
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
    out: &mut Stdout,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // Why is this so complicated?  The dir entry file_type() call is inexpensive,
    // so split into dirs and files here and then to paths, and must appropriately
    // handle symlinks to dirs to avoid recursive symlinks as well
    let (vec_dirs, vec_files): (Vec<PathBuf>, Vec<PathBuf>) = std::fs::read_dir(&requested_dir)?
        .flatten()
        .par_bridge()
        .partition_map(|dir_entry| {
            let path = dir_entry.path();
            match dir_entry.file_type() {
                Ok(file_type) => match file_type {
                    file_type if file_type.is_dir() => Either::Left(path),
                    file_type if file_type.is_file() => Either::Right(path),
                    file_type if file_type.is_symlink() => {
                        match path.read_link() {
                            Ok(link) => {
                                // read_link() will check symlink is pointing to a directory
                                //
                                // checking ancestors() against the read_link() will reduce/remove
                                // infinitely recursive paths, like /usr/bin/X11 pointing to /usr/X11
                                if link.is_dir()
                                    && link.ancestors().all(|ancestor| ancestor != link)
                                {
                                    Either::Left(path)
                                } else {
                                    Either::Right(path)
                                }
                            }
                            // we get an error? still pass the path on, as we get a good path from the dir entry
                            Err(_) => Either::Right(path),
                        }
                    }
                    // char, block, etc devices(?) to the right
                    _ => Either::Right(path),
                },
                // we get an error? still pass the path on, as we get a good path from the dir entry
                Err(_) => Either::Right(path),
            }
        });

    match config.exec_mode {
        ExecMode::Deleted => {
            let vec_deleted = get_deleted(&config, requested_dir)?;
            if vec_deleted.is_empty() {
                // Shows progress, while we are finding no deleted files
                if config.opt_recursive {
                    eprint!(".");
                }
            } else {
                let pseudo_live_versions: Vec<PathData> = vec_deleted
                    .par_iter()
                    .map(|path| path.path_buf.file_name())
                    .flatten()
                    .map(|file_name| requested_dir.join(file_name))
                    .map(|path| PathData::from(path.as_path()))
                    .collect();

                let output_buf = display_exec(&config, [vec_deleted, pseudo_live_versions])?;
                // have to get a line break here, but shouldn't look unnatural
                // print "." but don't print if in non-recursive mode
                if config.opt_recursive {
                    eprintln!(".");
                }
                write!(out, "{}", output_buf)?;
                out.flush()?;
            }
        }
        _ => {
            let get_pseudo_live_versions = |config: &Config,
                                            requested_dir: &Path|
             -> Result<
                Vec<PathBuf>,
                Box<dyn std::error::Error + Send + Sync + 'static>,
            > {
                let vec_deleted = get_deleted(config, requested_dir)?;
                let pseudo_live_versions: Vec<PathBuf> = vec_deleted
                    .par_iter()
                    .map(|path| path.path_buf.file_name())
                    .flatten()
                    .map(|file_name| requested_dir.join(file_name))
                    .collect();
                Ok(pseudo_live_versions)
            };

            // combine dirs and files into a vec and sort to display
            let mut combined_vec: Vec<PathBuf> = match config.deleted_mode {
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

            combined_vec.par_sort_unstable_by(|a, b| a.cmp(b));
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

pub fn paint_string(path: &Path, file_name: &str) -> String {
    let ls_colors = LsColors::from_env().unwrap_or_default();

    if let Some(style) = ls_colors.style_for_path(path) {
        let ansi_style = &Style::to_ansi_term_style(style);
        ansi_style.paint(file_name).to_string()
    } else {
        file_name.to_owned()
    }
}
