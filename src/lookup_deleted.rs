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
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::read_dir,
    path::{Path, PathBuf},
    time::SystemTime,
};

use itertools::Itertools;

use crate::lookup_versions::{
    get_version_search_bundles, select_search_datasets, RelativePathAndSnapMounts,
};
use crate::utility::{BasicDirEntryInfo, PathData};
use crate::{Config, HttmResult};

pub fn deleted_lookup_exec(
    config: &Config,
    requested_dir: &Path,
) -> HttmResult<Vec<BasicDirEntryInfo>> {
    // we always need a requesting dir because we are comparing the files in the
    // requesting dir to those of their relative dirs on snapshots
    let requested_dir_pathdata = PathData::from(requested_dir);
    let vec_requested_dir_pathdata = vec![&requested_dir_pathdata];

    // create vec of all local and replicated backups at once
    //
    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by filename and latest file version here
    let basic_dir_entry_info_iter = vec_requested_dir_pathdata
        .iter()
        .flat_map(|pathdata| {
            config
                .dataset_collection
                .snaps_selected_for_search
                .value()
                .iter()
                .flat_map(|dataset_type| select_search_datasets(config, pathdata, dataset_type))
                .flat_map(|datasets_of_interest| {
                    get_version_search_bundles(config, pathdata, &datasets_of_interest)
                })
        })
        .flatten()
        .flat_map(|search_bundle| {
            get_unique_deleted_for_dir(&requested_dir_pathdata.path_buf, &search_bundle)
        })
        .flatten();

    let unique_deleted = get_latest_in_time_for_filename(basic_dir_entry_info_iter)
        .map(|(_file_name, (_modify_time, basic_dir_entry_info))| basic_dir_entry_info)
        .collect();

    Ok(unique_deleted)
}

// this functions like a BTreeMap, separate into buckets/groups
// by file name, then return the oldest deleted dir entry, or max by its modify time
// why? because this might be a folder that has been deleted and we need some policy
// to give later functions an idea about which folder to choose when we want too look
// behind deleted dirs, here we just choose latest in time
fn get_latest_in_time_for_filename<I>(
    basic_dir_entry_info_iter: I,
) -> impl Iterator<Item = (OsString, (SystemTime, BasicDirEntryInfo))>
where
    I: Iterator<Item = BasicDirEntryInfo>,
{
    basic_dir_entry_info_iter
        .into_group_map_by(|basic_dir_entry_info| basic_dir_entry_info.file_name.clone())
        .into_iter()
        .flat_map(|(file_name, group_of_dir_entries)| {
            group_of_dir_entries
                .into_iter()
                .flat_map(|basic_dir_entry_info| {
                    basic_dir_entry_info
                        .get_modify_time()
                        .map(|modify_time| (modify_time, basic_dir_entry_info))
                })
                .max_by_key(|(modify_time, _basic_dir_entry_info)| *modify_time)
                .map(|latest_entry_in_time| (file_name, latest_entry_in_time))
        })
}

fn get_unique_deleted_for_dir(
    requested_dir: &Path,
    search_bundle: &RelativePathAndSnapMounts,
) -> HttmResult<Vec<BasicDirEntryInfo>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    //
    // create a collection of local file names
    let local_filenames_map: BTreeSet<OsString> = read_dir(&requested_dir)?
        .flatten()
        .map(|dir_entry| dir_entry.file_name())
        .collect();

    let unique_snap_filenames: BTreeMap<OsString, BasicDirEntryInfo> =
        get_unique_snap_filenames(&search_bundle.snap_mounts, &search_bundle.relative_path)?;

    // compare local filenames to all unique snap filenames - none values are unique, here
    let all_deleted_versions: Vec<BasicDirEntryInfo> = unique_snap_filenames
        .into_iter()
        .filter(|(file_name, _)| !local_filenames_map.contains(file_name))
        .map(|(_file_name, basic_dir_entry_info)| basic_dir_entry_info)
        .collect();

    Ok(all_deleted_versions)
}

fn get_unique_snap_filenames(
    mounts: &[PathBuf],
    relative_path: &Path,
) -> HttmResult<BTreeMap<OsString, BasicDirEntryInfo>> {
    let basic_dir_entry_info_iter = mounts
        .iter()
        .map(|path| path.join(&relative_path))
        .flat_map(|path| read_dir(&path))
        .flatten()
        .flatten()
        .map(|dir_entry| BasicDirEntryInfo::from(&dir_entry));

    // why do we care to check whether the dir entry is latest in time here as well as above?  because if we miss it here
    // the policy of latest in time would make no sense.  read_dir call could return mounts in no temporal order, and
    // entering into a map would leave only the last inserted in the map, not the latest in modify time
    let unique_snap_filenames = get_latest_in_time_for_filename(basic_dir_entry_info_iter)
        .map(|(file_name, latest_entry_in_time)| (file_name, latest_entry_in_time.1))
        .collect();
    Ok(unique_snap_filenames)
}
