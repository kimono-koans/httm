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

use crate::MapOfSnaps;
use crate::filesystem::aliases::MapOfAliases;
use crate::filesystem::alts::MapOfAlts;
use crate::filesystem::mounts::{
    BaseFilesystemInfo, FilesystemType, FilterDirs, MapOfDatasets, TM_DIR_LOCAL_PATH,
    TM_DIR_REMOTE_PATH,
};
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::find_common_path;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemInfo {
    // key: mount, val: (dataset/subvol, fs_type, mount_type)
    pub map_of_datasets: MapOfDatasets,
    // vec dirs to be filtered
    pub filter_dirs: FilterDirs,
    // key: mount, val: alt dataset
    pub opt_map_of_alts: Option<MapOfAlts>,
    // key: local dir, val: (remote dir, fstype)
    pub opt_map_of_aliases: Option<MapOfAliases>,
    // opt possible opt store type
    pub opt_alt_store: Option<FilesystemType>,
}

impl FilesystemInfo {
    pub fn new(
        opt_alt_replicated: bool,
        opt_remote_dir: Option<&String>,
        opt_local_dir: Option<&String>,
        opt_raw_aliases: Option<Vec<String>>,
        opt_alt_store: Option<FilesystemType>,
        pwd: PathBuf,
    ) -> HttmResult<FilesystemInfo> {
        let mut base_fs_info = BaseFilesystemInfo::new(&opt_alt_store)?;

        // only create a map of aliases if necessary (aliases conflicts with alt stores)
        let opt_map_of_aliases = if opt_raw_aliases.is_none() && opt_remote_dir.is_none() {
            None
        } else {
            MapOfAliases::new(
                &base_fs_info.map_of_datasets,
                opt_raw_aliases,
                opt_remote_dir,
                opt_local_dir,
                &pwd,
            )?
        };

        // prep any blob repos
        let mut opt_alt_store = opt_alt_store;

        match opt_alt_store {
            Some(ref repo_type) => {
                base_fs_info.from_blob_repo(&repo_type)?;
            }
            None if base_fs_info.map_of_datasets.is_empty() => {
                // auto enable time machine alt store on mac when no datasets available, no working aliases, and paths exist
                if cfg!(target_os = "macos")
                    && opt_map_of_aliases.is_none()
                    && TM_DIR_REMOTE_PATH.exists()
                    && TM_DIR_LOCAL_PATH.exists()
                {
                    opt_alt_store.replace(FilesystemType::Apfs);
                    base_fs_info.from_blob_repo(&FilesystemType::Apfs)?;
                } else {
                    return HttmError::new("httm could not find any valid datasets on the system.")
                        .into();
                }
            }
            _ => {}
        }

        // only create a map of alts if necessary
        let opt_map_of_alts = if opt_alt_replicated {
            Some(MapOfAlts::new(&base_fs_info.map_of_datasets))
        } else {
            None
        };

        Ok(FilesystemInfo {
            map_of_datasets: base_fs_info.map_of_datasets,
            filter_dirs: base_fs_info.filter_dirs,
            opt_map_of_alts,
            opt_map_of_aliases,
            opt_alt_store,
        })
    }

    // if we have some non-ZFS mounts, we check to see if there is a snap directory in common
    // so we can hide that common path from searches later
    pub fn common_snap_dir(&self, map_of_snaps: &MapOfSnaps) -> Option<Box<Path>> {
        let map_of_datasets: &MapOfDatasets = &self.map_of_datasets;

        let vec_snaps: Vec<&Box<Path>> = map_of_datasets
            .par_iter()
            .filter(|(_mount, dataset_info)| dataset_info.fs_type != FilesystemType::Zfs)
            .filter_map(|(mount, _dataset_info)| map_of_snaps.get(mount))
            .flatten()
            .collect();

        if vec_snaps.is_empty() {
            return None;
        }

        find_common_path(vec_snaps)
    }
}
