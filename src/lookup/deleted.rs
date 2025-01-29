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
use hashbrown::HashSet;
use std::ffi::OsString;
use std::fs::read_dir;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeletedFiles {
    inner: Vec<BasicDirEntryInfo>,
}

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

        let all_file_names = Self::all_file_names(requested_dir)?;

        let pseudo_live_versions = all_file_names
            .difference(&local_filenames_set)
            .into_iter()
            // this iter creates dummy "live versions" values to match deleted files
            // which have been found on snapshots, so we return to the user "the path that
            // once was" in their browse panel
            .map(|file_name| Self::into_pseudo_live_version(&file_name, requested_dir))
            .collect();

        Ok(Self {
            inner: pseudo_live_versions,
        })
    }

    #[inline(always)]
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn all_file_names<'a>(requested_dir: &'a Path) -> HttmResult<HashSet<OsString>> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::from(requested_dir);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique

        let unique_deleted_file_names_for_dir: HashSet<OsString> =
            ProximateDatasetAndOptAlts::new(&path_data)?
                .into_search_bundles()
                .flat_map(|search_bundle| Self::all_file_names_for_directory(search_bundle))
                .collect();

        Ok(unique_deleted_file_names_for_dir)
    }

    #[inline(always)]
    fn all_file_names_for_directory<'a>(
        search_bundle: RelativePathAndSnapMounts<'a>,
    ) -> impl Iterator<Item = OsString> + 'a {
        // compare local filenames to all unique snap filenames - none values are unique, here
        search_bundle
            .snap_mounts
            .into_owned()
            .into_iter()
            .map(|path| path.join(search_bundle.relative_path.as_os_str()))
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .map(|dir_entry| dir_entry.file_name())
    }

    // this function creates dummy "live versions" values to match deleted files
    // which have been found on snapshots, we return to the user "the path that
    // once was" in their browse panel
    #[inline(always)]
    fn into_pseudo_live_version(file_name: &OsString, pseudo_live_dir: &Path) -> BasicDirEntryInfo {
        let path = pseudo_live_dir.join(file_name);

        let opt_filetype = None;

        BasicDirEntryInfo::new(path, opt_filetype)
    }
}
