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
use hashbrown::HashSet;
use std::ffi::OsString;
use std::fs::FileType;
use std::path::Path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DeletedFiles {
    inner: HashSet<BasicDirEntryInfo>,
}

impl From<HashSet<BasicDirEntryInfo>> for DeletedFiles {
    fn from(value: HashSet<BasicDirEntryInfo>) -> Self {
        Self { inner: value }
    }
}

impl DeletedFiles {
    pub fn new(requested_dir: &Path) -> Self {
        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        let mut deleted_files: DeletedFiles =
            Self::unique_pseudo_live_versions(requested_dir).into();

        if deleted_files.inner.is_empty() {
            return deleted_files;
        }

        // this iter creates dummy "live versions" values to match deleted files
        // which have been found on snapshots, so we return to the user "the path that
        // once was" in their browse panel
        if let Ok(read_dir) = std::fs::read_dir(requested_dir) {
            let live_paths: HashSet<BasicDirEntryInfo> = read_dir
                .flatten()
                .map(|entry| BasicDirEntryInfo::from(entry))
                .collect_set_no_update();

            if live_paths.is_empty() {
                return deleted_files;
            }

            deleted_files.remove_live_paths(&live_paths);
        }

        deleted_files
    }

    #[inline(always)]
    fn remove_live_paths(&mut self, live_paths: &HashSet<BasicDirEntryInfo>) {
        live_paths.iter().for_each(|live_file| {
            let _ = self.inner.remove(live_file);
        });
    }

    #[inline(always)]
    pub fn into_inner(self) -> HashSet<BasicDirEntryInfo> {
        self.inner
    }

    #[inline(always)]
    fn unique_pseudo_live_versions<'a>(requested_dir: &'a Path) -> HashSet<BasicDirEntryInfo> {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let path_data = PathData::from(requested_dir);

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique

        let Ok(prox_opt_alts) = ProximateDatasetAndOptAlts::new(&path_data) else {
            return HashSet::new();
        };

        let unique_deleted_file_names_for_dir: HashSet<BasicDirEntryInfo> = prox_opt_alts
            .into_search_bundles()
            .flat_map(|search_bundle| {
                Self::all_snapshot_paths_for_directory(&requested_dir, search_bundle)
            })
            .collect_set_no_update();

        unique_deleted_file_names_for_dir
    }

    #[inline(always)]
    fn all_snapshot_paths_for_directory<'a>(
        pseudo_live_dir: &'a Path,
        search_bundle: RelativePathAndSnapMounts<'a>,
    ) -> impl Iterator<Item = BasicDirEntryInfo> + 'a {
        // compare local filenames to all unique snap filenames - none values are unique, here
        search_bundle
            .snap_mounts()
            .to_owned()
            .into_iter()
            .map(move |path| path.join(search_bundle.relative_path()))
            // important to note: this is a read dir on snapshots directories,
            // b/c read dir on deleted dirs from a live filesystem will fail
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .map(|dir_entry| {
                Self::into_pseudo_live_version(
                    dir_entry.file_name(),
                    pseudo_live_dir,
                    dir_entry.file_type().ok(),
                )
            })
    }

    // this function creates dummy "live versions" values to match deleted files
    // which have been found on snapshots, we return to the user "the path that
    // once was" in their browse panel
    #[inline(always)]
    fn into_pseudo_live_version<'a>(
        file_name: OsString,
        pseudo_live_dir: &'a Path,
        opt_file_type: Option<FileType>,
    ) -> BasicDirEntryInfo {
        let path = pseudo_live_dir.join(file_name);

        BasicDirEntryInfo::new(&path, opt_file_type)
    }
}
