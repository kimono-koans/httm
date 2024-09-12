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

use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::library::results::HttmResult;
use crate::lookup::versions::{ProximateDatasetAndOptAlts, RelativePathAndSnapMounts};
use hashbrown::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::read_dir;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeletedFiles {
    inner: Vec<BasicDirEntryInfo>,
}

// deleted lookup is a dumb impl. if we want to rank outputs, get last in time, etc.
// we do that elsewhere.  deleted is simply about finding at least one version of a deleted file
// this, believe it or not, will be faster
impl DeletedFiles {
    pub fn new(requested_dir: &Path) -> HttmResult<Self> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let requested_dir_pathdata = PathData::from(requested_dir);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique
        // as these will be the filenames that populate our interactive views, so deduplicate
        // by filename and latest file version here
        let basic_info_map: HashMap<OsString, BasicDirEntryInfo> =
            ProximateDatasetAndOptAlts::new(&requested_dir_pathdata)?
                .into_search_bundles()
                .flat_map(|search_bundle| {
                    Self::unique_deleted_for_dir(&requested_dir_pathdata.path(), &search_bundle)
                })
                .flatten()
                .map(|basic_info| (basic_info.filename().to_os_string(), basic_info))
                .collect();

        Ok(Self {
            inner: basic_info_map.into_values().collect(),
        })
    }

    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    fn unique_deleted_for_dir(
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
            Self::unique_snap_filenames(search_bundle.snap_mounts, search_bundle.relative_path);

        // compare local filenames to all unique snap filenames - none values are unique, here
        let all_deleted_versions = unique_snap_filenames
            .into_iter()
            .filter(move |(file_name, _basic_info)| !local_filenames_set.contains(file_name))
            .map(|(_file_name, basic_info)| basic_info);

        Ok(all_deleted_versions)
    }

    fn unique_snap_filenames(
        mounts: &[Box<Path>],
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
