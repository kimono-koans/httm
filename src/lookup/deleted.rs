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
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{
    ffi::OsString,
    fs::read_dir,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use hashbrown::{HashMap, HashSet};

use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::library::results::HttmResult;
use crate::lookup::versions::{RelativePathAndSnapMounts, VersionsMap};
use crate::GLOBAL_CONFIG;

pub struct DeletedFilesIter {
    inner: Box<dyn Iterator<Item = BasicDirEntryInfo>>,
}

impl Deref for DeletedFilesIter {
    type Target = Box<dyn Iterator<Item = BasicDirEntryInfo>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for DeletedFilesIter {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<&Path> for DeletedFilesIter {
    fn from(requested_dir: &Path) -> Self {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let requested_dir_pathdata = PathData::from(requested_dir);

        let requested_snap_datasets = GLOBAL_CONFIG
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique
        // as these will be the filenames that populate our interactive views, so deduplicate
        // by filename and latest file version here
        let basic_info_map: HashMap<OsString, BasicDirEntryInfo> =
            VersionsMap::get_search_bundles(&requested_dir_pathdata, requested_snap_datasets)
                .flat_map(|search_bundle| {
                    Self::get_unique_deleted_for_dir(
                        &requested_dir_pathdata.path_buf,
                        &search_bundle,
                    )
                })
                .flatten()
                .map(|basic_info| (basic_info.get_filename().to_os_string(), basic_info))
                .collect();

        Self {
            inner: Box::new(basic_info_map.into_values()),
        }
    }
}

// deleted_lookup_exec is a dumb function/module if we want to rank outputs, get last in time, etc.
// we do that elsewhere.  deleted is simply about finding at least one version of a deleted file
// this, believe it or not, will be faster
impl DeletedFilesIter {
    fn get_unique_deleted_for_dir(
        requested_dir: &Path,
        search_bundle: &RelativePathAndSnapMounts,
    ) -> HttmResult<impl Iterator<Item = BasicDirEntryInfo>> {
        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        let local_filenames_set: HashSet<OsString> = read_dir(requested_dir)?
            .flatten()
            .map(|dir_entry| dir_entry.file_name())
            .collect();

        let unique_snap_filenames: HashMap<OsString, BasicDirEntryInfo> =
            Self::get_unique_snap_filenames(search_bundle.snap_mounts, search_bundle.relative_path);

        // compare local filenames to all unique snap filenames - none values are unique, here
        let all_deleted_versions =
            unique_snap_filenames
                .into_iter()
                .filter_map(move |(file_name, basic_info)| {
                    if !local_filenames_set.contains(&file_name) {
                        Some(basic_info)
                    } else {
                        None
                    }
                });

        Ok(all_deleted_versions)
    }

    fn get_unique_snap_filenames(
        mounts: &[PathBuf],
        relative_path: &Path,
    ) -> HashMap<OsString, BasicDirEntryInfo> {
        mounts
            .iter()
            .map(|path| path.join(relative_path))
            .flat_map(read_dir)
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
            .collect::<HashMap<OsString, BasicDirEntryInfo>>()
    }
}

pub struct LastInTimeSet {
    inner: Box<dyn Iterator<Item = PathBuf>>,
}

impl Deref for LastInTimeSet {
    type Target = Box<dyn Iterator<Item = PathBuf>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for LastInTimeSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<Vec<PathData>> for LastInTimeSet {
    // this is very similar to VersionsMap, but of course returns only last in time
    // for directory paths during deleted searches.  it's important to have a policy, here,
    // last in time, for which directory we return during deleted searches, because
    // different snapshot-ed dirs may contain different files.

    // this fn is also missing parallel iter fns, to make the searches more responsive
    // by leaving parallel search for the interactive views
    fn from(path_set: Vec<PathData>) -> Self {
        // create vec of all local and replicated backups at once
        let snaps_selected_for_search = GLOBAL_CONFIG
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let unboxed = path_set.into_iter().filter_map(|pathdata| {
            VersionsMap::get_search_bundles(&pathdata, snaps_selected_for_search)
                .filter_map(|search_bundle| search_bundle.get_last_version())
                .max_by_key(|pathdata| pathdata.get_md_infallible().modify_time)
                .map(|pathdata| pathdata.path_buf)
        });

        Self {
            inner: Box::new(unboxed),
        }
    }
}
