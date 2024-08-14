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

use crate::library::results::HttmResult;
use crate::parse::aliases::MapOfAliases;
use crate::parse::alts::MapOfAlts;
use crate::parse::mounts::{BaseFilesystemInfo, FilesystemType, FilterDirs, MapOfDatasets};
use crate::parse::snaps::MapOfSnaps;
use clap::parser::RawValues;
use std::path::Path;
use std::path::PathBuf;

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
    pub opt_common_snap_dir: Option<PathBuf>,
}

impl FilesystemInfo {
    pub fn new(
        opt_alt_replicated: bool,
        opt_debug: bool,
        opt_remote_dir: Option<&str>,
        opt_local_dir: Option<&str>,
        opt_raw_aliases: Option<RawValues>,
        opt_alt_store: &mut Option<&FilesystemType>,
        pwd: &Path,
    ) -> HttmResult<FilesystemInfo> {
        // only create a map of aliases if necessary (aliases conflicts with alt stores)
        let opt_map_of_aliases =
            MapOfAliases::new(opt_raw_aliases, opt_remote_dir, opt_local_dir, pwd)?;

        let base_fs_info = BaseFilesystemInfo::new(opt_debug, opt_alt_store, &opt_map_of_aliases)?;

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
        })
    }
}
