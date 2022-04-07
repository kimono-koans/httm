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
use crate::lookup::{get_dataset, get_snap_and_local};

use crate::{Config, PathData};
use rayon::prelude::*;

use fxhash::FxHashMap as HashMap;
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
        let path = PathBuf::from(config.raw_paths.get(0).unwrap());
        let pathdata = PathData::new(config, &path);
        recursive_del_search(config, &pathdata, out)?;

        // exit successfully upon ending recursive search
        std::process::exit(0)
    } else {
        let path = PathBuf::from(config.raw_paths.get(0).unwrap());
        let pathdata_set = get_deleted(config, &path)?;

        Ok(vec![pathdata_set, Vec::new()])
    }
}

fn recursive_del_search(
    config: &Config,
    pathdata: &PathData,
    out: &mut Stdout,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let read_dir = std::fs::read_dir(&pathdata.path_buf)?;

    // convert to paths, and split into dirs and files
    let vec_dirs: Vec<PathBuf> = read_dir
        .into_iter()
        .par_bridge()
        .filter_map(|i| i.ok())
        .map(|dir_entry| dir_entry.path())
        .filter(|path| path.is_dir())
        .collect();

    let vec_deleted: Vec<PathData> = get_deleted(config, &pathdata.path_buf)?;

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
            let path = PathData::new(config, requested_dir);
            let _ = recursive_del_search(config, &path, out);
        });
    Ok(())
}

pub fn get_deleted(
    config: &Config,
    path: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pathdata = PathData::new(config, path);

    // which ZFS dataset do we want to use
    let dataset = if let Some(ref snap_point) = config.opt_snap_point {
        snap_point.to_owned()
    } else {
        get_dataset(&pathdata)?
    };

    // generates path for hidden .zfs snap dir, and the corresponding local path
    let (snapshot_dir, local_path) = get_snap_and_local(config, &pathdata, dataset)?;

    let local_dir_entries: Vec<DirEntry> = std::fs::read_dir(&pathdata.path_buf)?
        .into_iter()
        .par_bridge()
        .flatten()
        .collect();

    let mut local_unique_filenames: HashMap<OsString, PathBuf> = HashMap::default();

    let _ = local_dir_entries.iter().for_each(|dir_entry| {
        let stripped = dir_entry.file_name();
        let _ = local_unique_filenames.insert(stripped, dir_entry.path());
    });

    // Now we have to find all file names in the snap_dirs and compare against the local_dir
    let snap_files: Vec<(OsString, PathBuf)> = std::fs::read_dir(&snapshot_dir)?
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
    let _ = snap_files.into_iter().for_each(|(file_name, path)| {
        let _ = unique_snap_filenames.insert(file_name, path);
    });

    // deduplication by name - none values are unique here
    let deleted_pathdata: Vec<PathData> = unique_snap_filenames
        .par_iter()
        .filter(|(file_name, _)| local_unique_filenames.get(file_name.to_owned()).is_none())
        .map(|(_, path)| PathData::new(config, path))
        .collect();

    // deduplication by modify time and size - as we would elsewhere
    let mut unique_deleted_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    let _ = deleted_pathdata.into_iter().for_each(|pathdata| {
        let _ = unique_deleted_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<_> = unique_deleted_versions.into_iter().collect();

    sorted.par_sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}
