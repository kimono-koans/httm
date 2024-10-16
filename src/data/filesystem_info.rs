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

use crate::filesystem::aliases::MapOfAliases;
use crate::filesystem::alts::MapOfAlts;
use crate::filesystem::mounts::{
    BaseFilesystemInfo,
    FilesystemType,
    FilterDirs,
    MapOfDatasets,
    TM_DIR_LOCAL_PATH,
    TM_DIR_REMOTE_PATH,
};
use crate::filesystem::snaps::MapOfSnaps;
use crate::library::results::{HttmError, HttmResult};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemInfo {
    // key: mount, val: (dataset/subvol, fs_type, mount_type)
    pub map_of_datasets: MapOfDatasets,
    // key: mount, val: vec snap locations on disk (e.g. /.zfs/snapshot/snap_8a86e4fc_prepApt/home)
    pub map_of_snaps: MapOfSnaps,
    // vec dirs to be filtered
    pub filter_dirs: FilterDirs,
    // key: mount, val: alt dataset
    pub opt_map_of_alts: Option<MapOfAlts>,
    // key: local dir, val: (remote dir, fstype)
    pub opt_map_of_aliases: Option<MapOfAliases>,
    // opt single dir to to be filtered re: btrfs common snap dir
    pub opt_common_snap_dir: Option<Box<Path>>,
    // opt possible opt store type
    pub opt_alt_store: Option<FilesystemType>,
}

impl FilesystemInfo {
    pub fn new(
        opt_alt_replicated: bool,
        opt_debug: bool,
        opt_remote_dir: Option<&String>,
        opt_local_dir: Option<&String>,
        opt_raw_aliases: Option<Vec<String>>,
        opt_alt_store: Option<FilesystemType>,
        pwd: PathBuf,
    ) -> HttmResult<FilesystemInfo> {
        let mut base_fs_info = BaseFilesystemInfo::new(opt_debug, &opt_alt_store)?;

        // only create a map of aliases if necessary (aliases conflicts with alt stores)
        let opt_map_of_aliases = MapOfAliases::new(
            &mut base_fs_info,
            opt_raw_aliases,
            opt_remote_dir,
            opt_local_dir,
            &pwd,
        )?;

        // prep any blob repos
        let mut opt_alt_store = opt_alt_store;

        match opt_alt_store {
            Some(ref repo_type) => {
                base_fs_info.from_blob_repo(&repo_type, opt_debug)?;
            }
            None if base_fs_info.map_of_datasets.is_empty() => {
                // auto enable time machine alt store on mac when no datasets available, no working aliases, and paths exist
                if cfg!(target_os = "macos")
                    && opt_map_of_aliases.is_none()
                    && TM_DIR_REMOTE_PATH.exists()
                    && TM_DIR_LOCAL_PATH.exists()
                {
                    opt_alt_store.replace(FilesystemType::Apfs);
                    base_fs_info.from_blob_repo(&FilesystemType::Apfs, opt_debug)?;
                } else {
                    return Err(HttmError::new(
                        "httm could not find any valid datasets on the system.",
                    )
                    .into());
                }
            }
            _ => {}
        }

        // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
        let opt_common_snap_dir = base_fs_info.common_snap_dir();

        // only create a map of alts if necessary
        let opt_map_of_alts = if opt_alt_replicated {
            Some(MapOfAlts::new(&base_fs_info.map_of_datasets))
        } else {
            None
        };

        Ok(FilesystemInfo {
            map_of_datasets: base_fs_info.map_of_datasets,
            map_of_snaps: base_fs_info.map_of_snaps,
            filter_dirs: base_fs_info.filter_dirs,
            opt_map_of_alts,
            opt_common_snap_dir,
            opt_map_of_aliases,
            opt_alt_store,
        })
    }
}
