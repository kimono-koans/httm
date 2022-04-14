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

use crate::display::display_exec;
use crate::lookup::{get_dataset, get_snap_point_and_local_relative_path};
use crate::{Config, PathData, SnapPoint};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    ffi::OsString,
    fs::DirEntry,
    io::{Stdout, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub fn deleted_exec(
    config: &Config,
    out: &mut Stdout,
) -> Result<Vec<Vec<PathData>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    if config.opt_recursive {
        recursive_del_search(config, &config.requested_dir, out)?;

        // exit successfully upon ending recursive search
        std::process::exit(0)
    } else {
        let pathdata_set = get_deleted(config, &config.requested_dir.path_buf)?;

        // back to our main fn exec() to be printed, with an empty live set
        Ok(vec![pathdata_set, Vec::new()])
    }
}

fn recursive_del_search(
    config: &Config,
    requested_dir: &PathData,
    out: &mut Stdout,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let read_dir = std::fs::read_dir(&requested_dir.path_buf)?;

    // convert to paths, and split into dirs and files
    let vec_dirs: Vec<PathBuf> = read_dir
        .into_iter()
        .par_bridge()
        .filter_map(|i| i.ok())
        .map(|dir_entry| dir_entry.path())
        .filter(|path| path.is_dir())
        .collect();

    let vec_deleted: Vec<PathData> = get_deleted(config, &requested_dir.path_buf)?;

    if vec_deleted.is_empty() {
        // Shows progress, while we are finding no deleted files
        eprintln!("...");
    } else {
        let output_buf = display_exec(config, vec![vec_deleted, Vec::new()])?;
        write!(out, "{}", output_buf)?;
        out.flush()?;
    }

    // now recurse into those dirs as requested
    vec_dirs
        // don't want to a par_iter here because it will block and wait for all results, instead of
        // printing and recursing into the subsequent dirs
        .iter()
        .for_each(|requested_dir| {
            let path = PathData::new(&config.pwd, requested_dir);
            let _ = recursive_del_search(config, &path, out);
        });
    Ok(())
}

pub fn get_deleted(
    config: &Config,
    path: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // which ZFS dataset do we want to use
    let dataset = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => defined_dirs.snap_dir.to_owned(),
        SnapPoint::Native(native_commands) => {
            get_dataset(native_commands, &PathData::new(&config.pwd, path))?
        }
    };

    // generates path for hidden .zfs snap dir, and the corresponding local path
    let (hidden_snapshot_dir, local_path) =
        get_snap_point_and_local_relative_path(config, path, &dataset)?;

    let local_dir_entries: Vec<DirEntry> = std::fs::read_dir(&path)?
        .into_iter()
        .par_bridge()
        .flatten()
        .collect();

    let mut local_unique_filenames: HashMap<OsString, PathBuf> = HashMap::default();

    local_dir_entries.iter().for_each(|dir_entry| {
        let stripped = dir_entry.file_name();
        let _ = local_unique_filenames.insert(stripped, dir_entry.path());
    });

    // Now we have to find all file names in the snap_dirs and compare against the local_dir
    let snap_files: Vec<(OsString, PathBuf)> = std::fs::read_dir(&hidden_snapshot_dir)?
        .into_iter()
        .par_bridge()
        .flatten_iter()
        .map(|entry| entry.path())
        .map(|path| path.join(&local_path))
        .map(|path| std::fs::read_dir(&path))
        .flatten_iter()
        .flatten_iter()
        .flatten_iter()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry.path()))
        .collect();

    let mut unique_snap_filenames: HashMap<OsString, PathBuf> = HashMap::default();
    snap_files.into_iter().for_each(|(file_name, path)| {
        let _ = unique_snap_filenames.insert(file_name, path);
    });

    // deduplication by name - none values are unique here
    let deleted_pathdata: Vec<PathData> = unique_snap_filenames
        .par_iter()
        .filter(|(file_name, _)| local_unique_filenames.get(file_name.to_owned()).is_none())
        .map(|(_, path)| PathData::new(&config.pwd, path))
        .collect();

    // deduplication by modify time and size - as we would elsewhere
    let mut unique_deleted_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    deleted_pathdata.into_iter().for_each(|pathdata| {
        let _ = unique_deleted_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<_> = unique_deleted_versions.into_iter().collect();

    sorted.par_sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}
