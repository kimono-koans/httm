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

use crate::GLOBAL_CONFIG;
use crate::data::paths::{
    BasicDirEntryInfo,
    PathData,
};
use crate::library::iter_extensions::HttmIter;
use crate::lookup::versions::{
    ProximateDatasetAndOptAlts,
    RelativePathAndSnapMounts,
};
use hashbrown::{
    HashMap,
    HashSet,
};
use std::ffi::{
    OsStr,
    OsString,
};
use std::fs::FileType;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFiles {
    inner: Vec<BasicDirEntryInfo>,
}

impl Default for DeletedFiles {
    fn default() -> Self {
        Self { inner: Vec::new() }
    }
}

impl DeletedFiles {
    pub fn new(requested_dir: &Path) -> Self {
        // creates dummy "live versions" values to match deleted files
        // which have been found on snapshots, so we return to the user "the path that
        // once was" in their browse panel
        let Some(mut deleted_files) = Self::unique_pseudo_live_versions(requested_dir) else {
            return Self::default();
        };

        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        // dir may or may not still exist
        if let Ok(read_dir) = std::fs::read_dir(requested_dir) {
            let live_paths: HashSet<OsString> = unsafe {
                read_dir
                    .flatten()
                    .map(|entry| entry.file_name())
                    .collect_set_unique()
            };

            if !live_paths.is_empty() {
                deleted_files.retain(|k, _v| !live_paths.contains(k));
            }
        }

        let deleted_file_names = deleted_files.into_values().collect();

        Self {
            inner: deleted_file_names,
        }
    }

    #[inline(always)]
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn unique_pseudo_live_versions<'a>(
        requested_dir: &'a Path,
    ) -> Option<HashMap<OsString, BasicDirEntryInfo>> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::without_styling(requested_dir, None);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique

        let prox_opt_alts = ProximateDatasetAndOptAlts::new(&GLOBAL_CONFIG, &path_data).ok()?;

        let pseudo_live_versions: HashMap<OsString, BasicDirEntryInfo> = prox_opt_alts
            .into_search_bundles()
            .fold(HashMap::new(), |mut acc, search_bundle| {
                let iter = Self::deleted_paths_for_directory(&requested_dir, &search_bundle);

                if acc.is_empty() {
                    acc = iter.collect_map_bulk_build();
                } else {
                    acc.extend(iter);
                }

                acc
            });

        if pseudo_live_versions.is_empty() {
            return None;
        }

        Some(pseudo_live_versions)
    }

    #[inline(always)]
    fn deleted_paths_for_directory<'a>(
        pseudo_live_dir: &'a Path,
        search_bundle: &'a RelativePathAndSnapMounts<'a>,
    ) -> impl Iterator<Item = (OsString, BasicDirEntryInfo)> {
        // compare local filenames to all unique snap filenames - none values are unique, here
        search_bundle
            .snap_mounts()
            .iter()
            .map(move |path| path.join(search_bundle.relative_path()))
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
            })
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
