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
use crate::lookup::versions::{ProximateDatasetAndOptAlts, RelativePathAndSnapMounts};
use hashbrown::HashMap;
use hashbrown::HashSet;
use std::ffi::OsString;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeletedFiles {
    inner: Vec<BasicDirEntryInfo>,
}

impl DeletedFiles {
    pub fn new(requested_dir: &Path) -> Self {
        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names

        let all_file_names = Self::all_file_names(requested_dir);

        if all_file_names.is_empty() {
            return Self { inner: Vec::new() };
        }

        // this iter creates dummy "live versions" values to match deleted files
        // which have been found on snapshots, so we return to the user "the path that
        // once was" in their browse panel
        let pseudo_live_versions = Self::into_pseudo_live_version(all_file_names, requested_dir);

        let local_filenames_set: HashSet<BasicDirEntryInfo> = match std::fs::read_dir(requested_dir)
        {
            Ok(read_dir) => read_dir
                .flatten()
                .into_iter()
                .map(|entry| BasicDirEntryInfo::from(entry))
                .collect(),
            Err(_) => {
                return Self {
                    inner: pseudo_live_versions.collect(),
                }
            }
        };

        let difference = pseudo_live_versions
            .into_iter()
            .filter(|entry| !local_filenames_set.contains(entry))
            .collect();

        Self { inner: difference }
    }

    #[inline(always)]
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn all_file_names<'a>(requested_dir: &'a Path) -> HashMap<OsString, BasicDirEntryInfo> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::from(requested_dir);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique

        let Ok(prox_opt_alts) = ProximateDatasetAndOptAlts::new(&path_data) else {
            return HashMap::new();
        };

        let unique_deleted_file_names_for_dir: HashMap<OsString, BasicDirEntryInfo> = prox_opt_alts
            .into_search_bundles()
            .flat_map(|search_bundle| Self::all_file_names_for_directory(search_bundle))
            .collect();

        unique_deleted_file_names_for_dir
    }

    #[inline(always)]
    fn all_file_names_for_directory<'a>(
        search_bundle: RelativePathAndSnapMounts<'a>,
    ) -> impl Iterator<Item = (OsString, BasicDirEntryInfo)> + 'a {
        // compare local filenames to all unique snap filenames - none values are unique, here
        search_bundle
            .snap_mounts
            .into_owned()
            .into_iter()
            .map(|path| path.join(search_bundle.relative_path.as_os_str()))
            // important to note: this is a read dir on snapshots directories,
            // b/c read dir on deleted dirs from a live filesystem will fail
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(dir_entry)))
    }

    // this function creates dummy "live versions" values to match deleted files
    // which have been found on snapshots, we return to the user "the path that
    // once was" in their browse panel
    #[inline(always)]
    fn into_pseudo_live_version<'a>(
        map: HashMap<OsString, BasicDirEntryInfo>,
        pseudo_live_dir: &'a Path,
    ) -> impl Iterator<Item = BasicDirEntryInfo> + 'a {
        map.into_iter().map(|(file_name, basic_info)| {
            let path = pseudo_live_dir.join(file_name);

            let opt_filetype = *basic_info.opt_filetype();

            BasicDirEntryInfo::new(path, opt_filetype)
        })
    }
}
