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

use std::{ffi::OsString, fs::read_dir, path::Path, time::SystemTime};

use fxhash::FxHashMap as HashMap;
use itertools::Itertools;
use rayon::prelude::*;

use crate::lookup::{get_search_dirs, NativeDatasetType, SearchDirs};
use crate::{BasicDirEntryInfo, Config, PathData};

pub fn get_unique_deleted(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<BasicDirEntryInfo>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups on alt replicated sets if necessary
    let selected_datasets = if config.opt_alt_replicated {
        vec![
            NativeDatasetType::AltReplicated,
            NativeDatasetType::MostImmediate,
        ]
    } else {
        vec![NativeDatasetType::MostImmediate]
    };

    // we always need a requesting dir because we are comparing the files in the
    // requesting dir to those of their relative dirs on snapshots
    let requested_dir_pathdata = PathData::from(requested_dir);

    // create vec of all local and replicated backups at once
    //
    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by filename and latest file version here
    let prep_deleted: Vec<(SystemTime, BasicDirEntryInfo)> = vec![&requested_dir_pathdata]
        .into_par_iter()
        .flat_map(|pathdata| {
            selected_datasets
                .par_iter()
                .flat_map(|dataset_type| get_search_dirs(config, pathdata, dataset_type))
        })
        .flatten()
        .flat_map(|search_dirs| {
            get_deleted_per_dataset(&requested_dir_pathdata.path_buf, &search_dirs)
        })
        .flatten()
        .filter_map(
            |basic_dir_entry_info| match basic_dir_entry_info.path.symlink_metadata() {
                Ok(md) => Some((md, basic_dir_entry_info)),
                Err(_) => None,
            },
        )
        .filter_map(|(md, basic_dir_entry_info)| match md.modified() {
            Ok(modify_time) => Some((modify_time, basic_dir_entry_info)),
            Err(_) => None,
        })
        .collect();

    // this part right here functions like a hashmap, separate into buckets/groups
    // by file name, then return the oldest deleted dir entry, or max by its modify time
    let unique_deleted: Vec<BasicDirEntryInfo> = prep_deleted
        .into_iter()
        .group_by(|(_modify_time, basic_dir_entry_info)| basic_dir_entry_info.file_name.clone())
        .into_iter()
        .filter_map(|(_key, group)| {
            group
                .into_iter()
                .max_by_key(|(modify_time, _basic_dir_entry_info)| *modify_time)
        })
        .map(|(_modify_time, basic_dir_entry_info)| basic_dir_entry_info)
        .collect();

    Ok(unique_deleted)
}

pub fn get_deleted_per_dataset(
    requested_dir: &Path,
    search_dirs: &SearchDirs,
) -> Result<Vec<BasicDirEntryInfo>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    // create a collection of local unique file names
    let unique_local_filenames: HashMap<OsString, BasicDirEntryInfo> = read_dir(&requested_dir)?
        .flatten()
        .par_bridge()
        .map(|dir_entry| {
            (
                dir_entry.file_name(),
                BasicDirEntryInfo {
                    file_name: dir_entry.file_name(),
                    path: dir_entry.path(),
                    file_type: dir_entry.file_type().ok(),
                },
            )
        })
        .collect();

    // now create a collection of file names in the snap_dirs
    // create a list of unique filenames on snaps
    let unique_snap_filenames: HashMap<OsString, BasicDirEntryInfo> =
        read_dir(&search_dirs.hidden_snapshot_dir)?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path())
            .map(|path| path.join(&search_dirs.relative_path))
            .flat_map(|path| read_dir(&path))
            .flatten_iter()
            .flatten()
            .map(|dir_entry| {
                (
                    dir_entry.file_name(),
                    BasicDirEntryInfo {
                        file_name: dir_entry.file_name(),
                        path: dir_entry.path(),
                        file_type: dir_entry.file_type().ok(),
                    },
                )
            })
            .collect();

    // compare local filenames to all unique snap filenames - none values are unique here
    let all_deleted_versions: Vec<BasicDirEntryInfo> = unique_snap_filenames
        .into_par_iter()
        .filter(|(file_name, _)| unique_local_filenames.get(file_name).is_none())
        .map(|(_file_name, basic_dir_entry_info)| basic_dir_entry_info)
        .collect();

    Ok(all_deleted_versions)
}
