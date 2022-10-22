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
    ffi::OsStr,
    fs::canonicalize,
    path::{Path, PathBuf},
};

use clap::OsValues;
use rayon::prelude::*;

use crate::config::generate::{DeletedMode, ExecMode, InteractiveMode};
use crate::data::filesystem_map::{DatasetCollection, SnapsSelectedForSearch};
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{httm_is_dir, read_stdin};
use crate::parse::aliases::parse_aliases;
use crate::parse::alts::precompute_alt_replicated;
use crate::parse::mounts::{get_common_snap_dir, parse_mounts_exec};

pub fn get_pwd() -> HttmResult<PathData> {
    if let Ok(pwd) = std::env::current_dir() {
        if let Ok(path) = PathBuf::from(&pwd).canonicalize() {
            Ok(PathData::from(path.as_path()))
        } else {
            Err(
                HttmError::new("Could not obtain a canonical path for your working directory")
                    .into(),
            )
        }
    } else {
        Err(HttmError::new(
            "Working directory does not exist or your do not have permissions to access it.",
        )
        .into())
    }
}

pub fn get_paths(
    os_values: Option<OsValues>,
    exec_mode: &ExecMode,
    pwd: &PathData,
) -> HttmResult<Vec<PathData>> {
    let mut paths = if let Some(input_files) = os_values {
        input_files
            .par_bridge()
            .map(Path::new)
            // canonicalize() on a deleted relative path will not exist,
            // so we have to join with the pwd to make a path that
            // will exist on a snapshot
            .map(|path| canonicalize(path).unwrap_or_else(|_| pwd.clone().path_buf.join(path)))
            .map(|path| PathData::from(path.as_path()))
            .collect()
    } else {
        match exec_mode {
            // setting pwd as the path, here, keeps us from waiting on stdin when in certain modes
            //  is more like Interactive and DisplayRecursive in this respect in requiring only one
            // input, and waiting on one input from stdin is pretty silly
            ExecMode::Interactive(_) | ExecMode::DisplayRecursive(_) => {
                vec![pwd.clone()]
            }
            ExecMode::Display
            | ExecMode::SnapFileMount(_)
            | ExecMode::MountsForFiles
            | ExecMode::NumVersions(_) => read_stdin()?
                .par_iter()
                .map(|string| PathData::from(Path::new(&string)))
                .collect(),
        }
    };

    // deduplicate pathdata and sort if in display mode --
    // so input of ./.z* and ./.zshrc will only print ./.zshrc once
    paths = if paths.len() > 1 {
        paths.sort_unstable();
        // dedup needs to be sorted/ordered first to work (not like a BTreeMap)
        paths.dedup();

        paths
    } else {
        paths
    };

    Ok(paths)
}

pub fn get_opt_requested_dir(
    exec_mode: &mut ExecMode,
    deleted_mode: &mut Option<DeletedMode>,
    paths: &[PathData],
    pwd: &PathData,
) -> HttmResult<Option<PathData>> {
    let res = match exec_mode {
        ExecMode::Interactive(_) | ExecMode::DisplayRecursive(_) => {
            match paths.len() {
                0 => Some(pwd.clone()),
                1 => {
                    // safe to index as we know the paths len is 1
                    let pathdata = &paths[0];

                    // use our bespoke is_dir fn for determining whether a dir here see pub httm_is_dir
                    if httm_is_dir(pathdata) {
                        Some(pathdata.clone())
                    // and then we take all comers here because may be a deleted file that DNE on a live version
                    } else {
                        match exec_mode {
                            ExecMode::Interactive(ref interactive_mode) => {
                                match interactive_mode {
                                    InteractiveMode::Browse => {
                                        // doesn't make sense to have a non-dir in these modes
                                        return Err(HttmError::new(
                                                    "Path specified is not a directory, and therefore not suitable for browsing.",
                                                )
                                                .into());
                                    }
                                    InteractiveMode::Restore | InteractiveMode::Select => {
                                        // non-dir file will just cause us to skip the lookup phase
                                        None
                                    }
                                }
                            }
                            // silently disable DisplayRecursive when path given is not a directory
                            // switch to a standard Display mode
                            ExecMode::DisplayRecursive(_) => {
                                *exec_mode = ExecMode::Display;
                                *deleted_mode = None;
                                None
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                n if n > 1 => {
                    return Err(HttmError::new(
                        "May only specify one path in the display recursive or interactive modes.",
                    )
                    .into())
                }
                _ => {
                    unreachable!()
                }
            }
        }
        ExecMode::Display
        | ExecMode::SnapFileMount(_)
        | ExecMode::MountsForFiles
        | ExecMode::NumVersions(_) => {
            // in non-interactive mode / display mode, requested dir is just a file
            // like every other file and pwd must be the requested working dir.
            None
        }
    };
    Ok(res)
}

pub fn get_dataset_collection(
    os_alt_replicated: bool,
    os_remote_dir: Option<&OsStr>,
    os_local_dir: Option<&OsStr>,
    os_map_aliases: Option<OsValues>,
    pwd: &PathData,
    exec_mode: &ExecMode,
) -> HttmResult<DatasetCollection> {
    let (map_of_datasets, map_of_snaps, vec_of_filter_dirs) = parse_mounts_exec()?;

    // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
    let opt_common_snap_dir = get_common_snap_dir(&map_of_datasets, &map_of_snaps);

    // only create a map of alts if necessary
    let opt_map_of_alts = if os_alt_replicated {
        Some(precompute_alt_replicated(&map_of_datasets))
    } else {
        None
    };

    let alias_values: Option<Vec<String>> =
        if let Some(env_map_aliases) = std::env::var_os("HTTM_MAP_ALIASES") {
            Some(
                env_map_aliases
                    .to_string_lossy()
                    .split_terminator(',')
                    .map(|str| str.to_owned())
                    .collect(),
            )
        } else {
            os_map_aliases.map(|cmd_map_aliases| {
                cmd_map_aliases
                    .into_iter()
                    .map(|os_str| os_str.to_string_lossy().to_string())
                    .collect()
            })
        };

    let raw_snap_dir = if let Some(value) = os_remote_dir {
        Some(value.to_os_string())
    } else if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
        std::env::var_os("HTTM_REMOTE_DIR")
    } else {
        // legacy env var name
        std::env::var_os("HTTM_SNAP_POINT")
    };

    let opt_map_of_aliases = if raw_snap_dir.is_some() || alias_values.is_some() {
        let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");

        let raw_local_dir = if let Some(value) = os_local_dir {
            Some(value.to_os_string())
        } else {
            env_local_dir
        };

        Some(parse_aliases(
            &raw_snap_dir,
            &raw_local_dir,
            pwd.path_buf.as_path(),
            &alias_values,
        )?)
    } else {
        None
    };

    // don't want to request alt replicated mounts in snap mode
    let snaps_selected_for_search =
        if os_alt_replicated && !matches!(exec_mode, ExecMode::SnapFileMount(_)) {
            SnapsSelectedForSearch::IncludeAltReplicated
        } else {
            SnapsSelectedForSearch::MostProximateOnly
        };

    Ok(DatasetCollection {
        map_of_datasets,
        map_of_snaps,
        opt_map_of_alts,
        vec_of_filter_dirs,
        opt_common_snap_dir,
        opt_map_of_aliases,
        snaps_selected_for_search,
    })
}
