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
use crate::lookup::{get_snap_point_and_local_relative_path, get_snapshot_dataset};
use crate::{Config, PathData, SnapPoint};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    ffi::OsString,
    fs::DirEntry,
    io::{Stdout, Write},
    path::Path,
    time::SystemTime,
};

pub fn deleted_exec(
    config: &Config,
    out: &mut Stdout,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // if recursive mode or if one path is directory path is given do a deleted search
    if config.requested_dir_mode || config.opt_recursive {
        deleted_search(config, &config.requested_dir, out)?;

        // flush and exit successfully upon ending recursive search
        if config.opt_recursive {
            println!();
            out.flush()?;
        }
        std::process::exit(0)
    } else {
        let pathdata_set = get_deleted(config, &config.requested_dir.path_buf)?;

        // back to our main fn exec() to be printed, with an empty live set
        Ok([pathdata_set, Vec::new()])
    }
}

fn deleted_search(
    config: &Config,
    requested_dir: &PathData,
    out: &mut Stdout,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // convert to paths, and split into dirs and files
    let vec_dirs: Vec<DirEntry> = std::fs::read_dir(&requested_dir.path_buf)?
        .into_iter()
        .par_bridge()
        .flatten_iter()
        .filter(|dir_entry| dir_entry.file_type().is_ok())
        .filter(|dir_entry| dir_entry.file_type().unwrap().is_dir())
        .collect();

    let vec_deleted: Vec<PathData> = get_deleted(config, &requested_dir.path_buf)?;

    if vec_deleted.is_empty() {
        // Shows progress, while we are finding no deleted files
        if config.opt_recursive {
            eprint!(".");
        }
    } else {
        let output_buf = display_exec(config, [vec_deleted, Vec::new()])?;
        // have to get a line break here, but shouldn't look unnatural
        // print "." but don't print if in non-recursive mode
        if config.opt_recursive {
            eprintln!(".");
        }
        write!(out, "{}", output_buf)?;
        out.flush()?;
    }

    // now recurse into those dirs as requested
    if config.opt_recursive {
        vec_dirs
            // don't want to a par_iter here because it will block and wait for all results, instead of
            // printing and recursing into the subsequent dirs
            .iter()
            .for_each(move |requested_dir| {
                let path = PathData::from(requested_dir);
                let _ = deleted_search(config, &path, out);
            });
    }

    Ok(())
}

pub fn get_deleted(
    config: &Config,
    path: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // which ZFS dataset do we want to use
    let dataset = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => defined_dirs.snap_dir.to_owned(),
        SnapPoint::Native(all_zfs_filesystems) => {
            get_snapshot_dataset(&PathData::from(path), all_zfs_filesystems)?
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

    // create a collection of local unique file names
    let mut local_unique_filenames: HashMap<OsString, DirEntry> = HashMap::default();
    local_dir_entries.into_iter().for_each(|dir_entry| {
        let _ = local_unique_filenames.insert(dir_entry.file_name(), dir_entry);
    });

    // now create a collection of file names in the snap_dirs
    let snap_files: Vec<(OsString, DirEntry)> = std::fs::read_dir(&hidden_snapshot_dir)?
        .into_iter()
        .par_bridge()
        .flatten_iter()
        .map(|entry| entry.path())
        .map(|path| path.join(&local_path))
        .map(|path| std::fs::read_dir(&path))
        .flatten_iter()
        .flatten_iter()
        .flatten_iter()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry))
        .collect();

    let mut unique_snap_filenames: HashMap<OsString, DirEntry> = HashMap::default();
    snap_files.into_iter().for_each(|(file_name, dir_entry)| {
        let _ = unique_snap_filenames.insert(file_name, dir_entry);
    });

    // compare local filenames to all unique snap filenames - none values are unique here
    let deleted_pathdata = unique_snap_filenames
        .into_iter()
        .filter(|(file_name, _)| local_unique_filenames.get(file_name).is_none())
        .map(|(_, dir_entry)| PathData::from(&dir_entry));

    // deduplicate all by modify time and size - as we would elsewhere
    let mut unique_deleted_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    deleted_pathdata.into_iter().for_each(|pathdata| {
        let _ = unique_deleted_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<_> = unique_deleted_versions.into_iter().collect();

    sorted.par_sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}
