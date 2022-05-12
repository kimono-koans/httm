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

use crate::lookup::{get_search_dirs, NativeDatasetType, SearchDirs};
use crate::{Config, PathData};

use fxhash::FxHashMap as HashMap;
use itertools::Itertools;
use std::{
    ffi::OsString,
    fs::{read_dir, DirEntry},
    path::Path,
};

#[allow(clippy::manual_map)]
pub fn get_unique_deleted(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<DirEntry>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups on alt replicated sets if necessary
    let selected_datasets = if config.opt_alt_replicated {
        vec![
            NativeDatasetType::AltReplicated,
            NativeDatasetType::MostImmediate,
        ]
    } else {
        vec![NativeDatasetType::MostImmediate]
    };

    let pathdata = PathData::from(requested_dir);

    // create vec of all local and replicated backups at once
    //
    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by filename and latest file version here
    let unique_deleted: Vec<DirEntry> = vec![&pathdata]
        .iter()
        .flat_map(|pathdata| {
            selected_datasets
                .iter()
                .flat_map(|dataset_type| get_search_dirs(config, pathdata, dataset_type))
        })
        .flatten()
        .flat_map(|search_dirs| get_deleted_per_dataset(&pathdata.path_buf, &search_dirs))
        .flatten()
        .filter_map(|dir_entry| match dir_entry.metadata() {
            Ok(md) => Some((md, dir_entry)),
            Err(_) => None,
        })
        .filter_map(|(md, dir_entry)| match md.modified() {
            Ok(modify_time) => Some((modify_time, dir_entry)),
            Err(_) => None,
        })
        .group_by(|(_modify_time, dir_entry)| dir_entry.file_name())
        .into_iter()
        .filter_map(|(_key, group)| {
            group
                .into_iter()
                .max_by_key(|(modify_time, _dir_entry)| modify_time.to_owned())
        })
        .map(|(_modify_time, dir_entry)| dir_entry)
        .collect();

    Ok(unique_deleted)
}

pub fn get_deleted_per_dataset(
    path: &Path,
    search_dirs: &SearchDirs,
) -> Result<Vec<DirEntry>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    // create a collection of local unique file names
    let unique_local_filenames: HashMap<OsString, DirEntry> = read_dir(&path)?
        .flatten()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry))
        .collect();

    // now create a collection of file names in the snap_dirs
    // create a list of unique filenames on snaps
    let unique_snap_filenames: HashMap<OsString, DirEntry> =
        read_dir(&search_dirs.hidden_snapshot_dir)?
            .flatten()
            .map(|entry| entry.path())
            .map(|path| path.join(&search_dirs.relative_path))
            .flat_map(|path| read_dir(&path))
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), dir_entry))
            .collect();

    // compare local filenames to all unique snap filenames - none values are unique here
    let unique_deleted_versions: Vec<DirEntry> = unique_snap_filenames
        .into_iter()
        .filter(|(file_name, _)| unique_local_filenames.get(file_name).is_none())
        .map(|(_file_name, dir_entry)| dir_entry)
        .collect();

    Ok(unique_deleted_versions)
}
