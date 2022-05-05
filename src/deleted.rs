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

use crate::lookup::{get_search_dirs, DatasetType, SearchDirs};
use crate::{Config, PathData};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    ffi::OsString,
    fs::{read_dir, DirEntry},
    path::Path,
    sync::Arc,
    time::SystemTime,
};

pub fn get_deleted(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups
    let selected_datasets = if config.opt_alt_replicated {
        Arc::new(vec![DatasetType::AltReplicated, DatasetType::MostImmediate])
    } else {
        Arc::new(vec![DatasetType::MostImmediate])
    };

    // create vec of all local and replicated backups at once
    let combined_deleted: Vec<PathData> = vec![PathData::from(requested_dir)]
        .par_iter()
        .map(|path_data| {
            selected_datasets
                .par_iter()
                .map(move |dataset_type| get_search_dirs(config, path_data, dataset_type))
                .flatten()
        })
        .into_par_iter()
        .flatten()
        .flatten_iter()
        .flat_map(|search_dirs| get_deleted_per_dataset(requested_dir, &search_dirs))
        .flatten()
        .collect();

    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by system time and size here
    let unique_deleted = if config.opt_alt_replicated {
        let unique_deleted: HashMap<(SystemTime, u64), PathData> = combined_deleted
            .into_par_iter()
            .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
            .collect();

        unique_deleted.into_par_iter().map(|(_, v)| v).collect()
    } else {
        combined_deleted
    };

    Ok(unique_deleted)
}

fn get_deleted_per_dataset(
    path: &Path,
    search_dirs: &SearchDirs,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    // create a collection of local unique file names
    let unique_local_filenames: HashMap<OsString, DirEntry> = read_dir(&path)?
        .flatten()
        .par_bridge()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry))
        .collect();

    // now create a collection of file names in the snap_dirs
    // create a list of unique filenames on snaps
    let unique_snap_filenames: HashMap<OsString, DirEntry> =
        read_dir(&search_dirs.hidden_snapshot_dir)?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path())
            .map(|path| path.join(&search_dirs.diff_path))
            .map(|path| read_dir(&path))
            .flatten_iter()
            .flatten_iter()
            .flatten_iter()
            .map(|dir_entry| (dir_entry.file_name(), dir_entry))
            .collect();

    // compare local filenames to all unique snap filenames - none values are unique here
    // deduplicate all by modify time and size - as we would elsewhere
    let unique_deleted_versions: HashMap<(SystemTime, u64), PathData> = unique_snap_filenames
        .into_par_iter()
        .filter(|(file_name, _)| unique_local_filenames.get(file_name).is_none())
        .map(|(_, dir_entry)| PathData::from(&dir_entry))
        .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
        .collect();

    let res_vec: Vec<_> = unique_deleted_versions
        .into_par_iter()
        .map(|(_, v)| v)
        .collect();

    Ok(res_vec)
}
