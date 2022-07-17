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
    ffi::OsString,
    fs::read_dir,
    path::{Path, PathBuf},
};

use itertools::Itertools;

use crate::utility::{BasicDirEntryInfo, HttmError, PathData};
use crate::versions_lookup::{
    get_datasets_for_search, get_search_bundle, prep_lookup_read_dir, SearchBundle,
};
use crate::{AHashMap as HashMap, Config, DatasetCollection, FilesystemType};

pub fn deleted_lookup_exec(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<BasicDirEntryInfo>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // we always need a requesting dir because we are comparing the files in the
    // requesting dir to those of their relative dirs on snapshots
    let requested_dir_pathdata = PathData::from(requested_dir);

    // create vec of all local and replicated backups at once
    //
    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by filename and latest file version here
    let unique_deleted: Vec<BasicDirEntryInfo> = vec![&requested_dir_pathdata]
        .iter()
        .flat_map(|pathdata| {
            config.selected_datasets.iter().flat_map(|dataset_type| {
                let datasets_for_search = get_datasets_for_search(config, pathdata, dataset_type)?;
                get_search_bundle(config, pathdata, &datasets_for_search)
            })
        })
        .flatten()
        .flat_map(|search_bundle| {
            get_deleted_per_dataset(config, &requested_dir_pathdata.path_buf, &search_bundle)
        })
        .flatten()
        .flat_map(|basic_dir_entry_info| {
            basic_dir_entry_info
                .path
                .symlink_metadata()
                .map(|md| (md, basic_dir_entry_info))
        })
        .flat_map(|(md, basic_dir_entry_info)| {
            md.modified()
                .map(|modify_time| (modify_time, basic_dir_entry_info))
        })
        // this functions like a hashmap, separate into buckets/groups
        // by file name, then return the oldest deleted dir entry, or max by its modify time
        // why? because this might be a folder that has been deleted and we need some policy
        // to give later functions an idea about which folder to choose when we want too look
        // behind deleted dirs, here we just choose latest in time
        .into_group_map_by(|(_modify_time, basic_dir_entry_info)| {
            basic_dir_entry_info.file_name.clone()
        })
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

fn get_deleted_per_dataset(
    config: &Config,
    requested_dir: &Path,
    search_bundle: &SearchBundle,
) -> Result<Vec<BasicDirEntryInfo>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    //
    // create a collection of local file names
    let local_filenames_map: HashMap<OsString, BasicDirEntryInfo> = read_dir(&requested_dir)?
        .flatten()
        .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
        .collect();

    // now create a collection of file names in the snap_dirs
    // create a list of unique filenames on snaps

    // this is the fallback way of handling without a map_of_snaps, if all we have is user defined dirs
    fn snap_filenames_from_read_dir(
        snapshot_dir: &Path,
        relative_path: &Path,
        fs_type: &FilesystemType,
    ) -> Result<
        HashMap<OsString, BasicDirEntryInfo>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_snap_filenames = prep_lookup_read_dir(snapshot_dir, relative_path, fs_type)?
            .iter()
            .flat_map(|joined_path| read_dir(&joined_path))
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
            .collect();

        Ok(unique_snap_filenames)
    }

    // this is the optimal way to handle for native datasets, if you have a map_of_snaps
    fn snap_filenames_from_snap_mounts(
        snap_mounts: &[PathBuf],
        relative_path: &Path,
    ) -> Result<
        HashMap<OsString, BasicDirEntryInfo>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_snap_filenames = snap_mounts
            .iter()
            .map(|path| path.join(&relative_path))
            .flat_map(|path| read_dir(&path))
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
            .collect();
        Ok(unique_snap_filenames)
    }

    let (snapshot_dir, relative_path, snapshot_mounts, fs_type) = {
        (
            &search_bundle.snapshot_dir,
            &search_bundle.relative_path,
            &search_bundle.snapshot_mounts,
            &search_bundle.fs_type,
        )
    };

    let unique_snap_filenames: HashMap<OsString, BasicDirEntryInfo> =
        match &config.dataset_collection {
            DatasetCollection::AutoDetect(_) => match snapshot_mounts {
                Some(snap_mounts) => snap_filenames_from_snap_mounts(snap_mounts, relative_path)?,
                None => {
                    return Err(HttmError::new(
                        "If you are here, precompute showed no snap mounts for dataset.  \
                    Iterator should just ignore/flatten the error.",
                    )
                    .into());
                }
            },
            DatasetCollection::UserDefined(_) => {
                snap_filenames_from_read_dir(snapshot_dir, relative_path, fs_type)?
            }
        };

    // compare local filenames to all unique snap filenames - none values are unique, here
    let all_deleted_versions: Vec<BasicDirEntryInfo> = unique_snap_filenames
        .into_iter()
        .filter(|(file_name, _)| !local_filenames_map.contains_key(file_name))
        .map(|(_file_name, basic_dir_entry_info)| basic_dir_entry_info)
        .collect();

    Ok(all_deleted_versions)
}
