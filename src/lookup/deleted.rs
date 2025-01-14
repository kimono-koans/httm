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
        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        let local_filenames_set: HashSet<OsString> = read_dir(requested_dir)?
            .flatten()
            .map(|dir_entry| dir_entry.file_name())
            .collect();

        let inner = Self::unique_deleted_for_dir(requested_dir, &local_filenames_set)?;

        Ok(Self { inner })
    }

    #[inline(always)]
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn unique_deleted_for_dir<'a>(
        requested_dir: &'a Path,
        local_filenames_set: &'a HashSet<OsString>,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::from(requested_dir);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique
        let unique_deleted_for_dir: HashMap<OsString, BasicDirEntryInfo> =
            ProximateDatasetAndOptAlts::new(&path_data)?
                .into_search_bundles()
                .flat_map(|search_bundle| {
                    Self::deleted_files_for_dataset(search_bundle, &local_filenames_set)
                })
                .collect();

        Ok(unique_deleted_for_dir.into_values().collect())
    }

    #[inline(always)]
    fn deleted_files_for_dataset<'a>(
        search_bundle: RelativePathAndSnapMounts<'a>,
        local_filenames_set: &'a HashSet<OsString>,
    ) -> impl Iterator<Item = (OsString, BasicDirEntryInfo)> + 'a {
        // compare local filenames to all unique snap filenames - none values are unique, here
        search_bundle
            .snap_mounts
            .into_iter()
            .map(|path| path.join(search_bundle.relative_path.as_os_str()))
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .filter(|dir_entry| !local_filenames_set.contains(&dir_entry.file_name()))
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
    }
}
