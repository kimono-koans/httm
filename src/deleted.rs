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

use crate::library::enumerate_directory;
use crate::lookup::get_search_dirs;
use crate::{Config, PathData};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use skim::prelude::*;
use std::{
    ffi::OsString,
    fs::DirEntry,
    io::{Stdout, Write},
    path::Path,
    sync::Arc,
    time::SystemTime,
};

pub fn deleted_exec(
    config: &Config,
    out: &mut Stdout,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
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

pub fn get_deleted(
    config: &Config,
    path: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // generates path for hidden .zfs snap dir, and the corresponding local path
    let (hidden_snapshot_dir, local_path) = get_search_dirs(config, &PathData::from(path), false)?;

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
        .flatten()
        .par_bridge()
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

    sorted.par_sort_unstable_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}
