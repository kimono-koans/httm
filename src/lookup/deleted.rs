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
use crate::library::iter_extensions::HttmIter;
use crate::lookup::versions::{ProximateDatasetAndOptAlts, RelativePathAndSnapMounts};
use hashbrown::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::FileType;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeletedFiles {
    inner: Vec<BasicDirEntryInfo>,
}

impl Default for DeletedFiles {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}

impl From<&Path> for DeletedFiles {
    fn from(requested_dir: &Path) -> Self {
        // creates dummy "live versions" values to match deleted files
        // which have been found on snapshots, so we return to the user "the path that
        // once was" in their browse panel
        let mut deleted_files: HashMap<OsString, BasicDirEntryInfo> =
            Self::unique_pseudo_live_versions(requested_dir);

        if deleted_files.is_empty() {
            return Self::default();
        }

        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        // dir may or may not still exist
        if let Ok(read_dir) = std::fs::read_dir(requested_dir) {
            let iter = read_dir.flatten().map(|entry| entry.file_name());

            // SAFETY: Known safe as single directory cannot contain same file names
            let live_paths = unsafe { iter.collect_set_unique() };

            if live_paths.is_empty() {
                return Self::default();
            }

            deleted_files.retain(|k, _v| !live_paths.contains(k));
        }

        let deleted_file_names = deleted_files.into_values().collect();

        Self {
            inner: deleted_file_names,
        }
    }
}

impl DeletedFiles {
    #[inline(always)]
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn unique_pseudo_live_versions<'a>(
        requested_dir: &'a Path,
    ) -> HashMap<OsString, BasicDirEntryInfo> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::without_styling(requested_dir, None);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique

        let Ok(prox_opt_alts) = ProximateDatasetAndOptAlts::new(&path_data) else {
            return HashMap::new();
        };

        prox_opt_alts
            .into_search_bundles()
            .map(|search_bundle| Self::snapshot_paths_for_directory(&requested_dir, search_bundle))
            .reduce(|mut acc, next| {
                acc.extend(next);
                acc
            })
            .unwrap_or_default()
    }

    #[inline(always)]
    fn snapshot_paths_for_directory<'a>(
        pseudo_live_dir: &'a Path,
        search_bundle: RelativePathAndSnapMounts<'a>,
    ) -> HashMap<OsString, BasicDirEntryInfo> {
        // compare local filenames to all unique snap filenames - none values are unique, here
        let iter = search_bundle
            .snap_mounts()
            .into_iter()
            .map(|path| path.join(search_bundle.relative_path()))
            // important to note: this is a read dir on snapshots directories,
            // b/c read dir on deleted dirs from a live filesystem will fail
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .filter_map(|dir_entry| {
                dir_entry
                    .file_type()
                    .ok()
                    .map(|file_type| (dir_entry, file_type))
            })
            .map(|(dir_entry, file_type)| {
                let file_name = dir_entry.file_name();
                let basic_info =
                    Self::into_pseudo_live_version(&file_name, pseudo_live_dir, Some(file_type));

                (file_name, basic_info)
            });

        // SAFETY: Known safe as single directory cannot contain same file names
        unsafe { iter.collect_map_unique() }
    }

    // this function creates dummy "live versions" values to match deleted files
    // which have been found on snapshots, we return to the user "the path that
    // once was" in their browse panel
    #[inline(always)]
    fn into_pseudo_live_version<'a>(
        file_name: &OsStr,
        pseudo_live_dir: &'a Path,
        opt_filetype: Option<FileType>,
    ) -> BasicDirEntryInfo {
        let path = pseudo_live_dir.join(file_name);

        BasicDirEntryInfo::new(&path, opt_filetype)
    }
}
