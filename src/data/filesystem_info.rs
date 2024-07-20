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
use crate::parse::mounts::{BaseFilesystemInfo, FilterDirs, MapOfDatasets};
use crate::parse::snaps::MapOfSnaps;
use clap::parser::RawValues;
use std::ffi::OsString;
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
    pub fn new<'a, 'b: 'a>(
        opt_alt_replicated: bool,
        opt_debug: bool,
        opt_remote_dir: Option<&&str>,
        opt_local_dir: Option<&&str>,
        opt_map_aliases: Option<RawValues>,
        opt_alt_backup: Option<&&str>,
        pwd: &Path,
    ) -> HttmResult<FilesystemInfo> {
        let base_fs_info = BaseFilesystemInfo::new(opt_debug, opt_alt_backup)?;

        // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
        let opt_common_snap_dir = base_fs_info.common_snap_dir();

        // only create a map of alts if necessary
        let opt_map_of_alts = if opt_alt_replicated {
            Some(MapOfAlts::new(&base_fs_info.map_of_datasets))
        } else {
            None
        };

        let raw_snap_dir = if let Some(value) = opt_remote_dir {
            Some(OsString::from(value))
        } else if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
            std::env::var_os("HTTM_REMOTE_DIR")
        } else {
            // legacy env var name
            std::env::var_os("HTTM_SNAP_POINT")
        };

        let alias_values: Option<Vec<String>> = match std::env::var_os("HTTM_MAP_ALIASES") {
            Some(env_map_alias) => Some(
                env_map_alias
                    .to_string_lossy()
                    .split_terminator(',')
                    .map(|s| s.to_owned())
                    .collect(),
            ),
            None => opt_map_aliases.map(|map_aliases| {
                map_aliases
                    .map(|os_str| os_str.to_string_lossy().to_string())
                    .collect()
            }),
        };

        let opt_map_of_aliases = if raw_snap_dir.is_some() || alias_values.is_some() {
            let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");

            let raw_local_dir = if let Some(value) = opt_local_dir {
                Some(OsString::from(value))
            } else {
                env_local_dir
            };

            Some(MapOfAliases::new(
                &raw_snap_dir,
                &raw_local_dir,
                pwd,
                &alias_values,
            )?)
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
