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
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{ffi::OsStr, path::PathBuf};

use clap::OsValues;
use rayon::prelude::*;

use crate::config::generate::ExecMode;
use crate::data::paths::PathData;
use crate::library::results::HttmResult;
use crate::library::utility::get_common_path;
use crate::lookup::versions::SnapsSelectedForSearch;
use crate::parse::aliases::{FilesystemType, MapOfAliases};
use crate::parse::alts::MapOfAlts;
use crate::parse::mounts::{BaseFilesystemInfo, FilterDirs, MapOfDatasets};
use crate::parse::snaps::MapOfSnaps;

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
    // vec of two enum variants - most proximate and alt replicated, or just most proximate
    pub snaps_selected_for_search: SnapsSelectedForSearch,
}

impl FilesystemInfo {
    pub fn new(
        opt_alt_replicated: bool,
        opt_remote_dir: Option<&OsStr>,
        opt_local_dir: Option<&OsStr>,
        opt_map_aliases: Option<OsValues>,
        pwd: &PathData,
        exec_mode: &ExecMode,
    ) -> HttmResult<FilesystemInfo> {
        let base_fs_info = BaseFilesystemInfo::new()?;

        // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
        let opt_common_snap_dir =
            get_common_snap_dir(&base_fs_info.map_of_datasets, &base_fs_info.map_of_snaps);

        // only create a map of alts if necessary
        let opt_map_of_alts = if opt_alt_replicated {
            Some(MapOfAlts::new(&base_fs_info.map_of_datasets))
        } else {
            None
        };

        let alias_values: Option<Vec<String>> =
            if let Some(env_map_aliases) = std::env::var_os("HTTM_MAP_ALIASES") {
                Some(
                    env_map_aliases
                        .to_string_lossy()
                        .split_terminator(',')
                        .map(std::borrow::ToOwned::to_owned)
                        .collect(),
                )
            } else {
                opt_map_aliases.map(|cmd_map_aliases| {
                    cmd_map_aliases
                        .into_iter()
                        .map(|os_str| os_str.to_string_lossy().to_string())
                        .collect()
                })
            };

        let raw_snap_dir = if let Some(value) = opt_remote_dir {
            Some(value.to_os_string())
        } else if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
            std::env::var_os("HTTM_REMOTE_DIR")
        } else {
            // legacy env var name
            std::env::var_os("HTTM_SNAP_POINT")
        };

        let opt_map_of_aliases = if raw_snap_dir.is_some() || alias_values.is_some() {
            let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");

            let raw_local_dir = if let Some(value) = opt_local_dir {
                Some(value.to_os_string())
            } else {
                env_local_dir
            };

            Some(MapOfAliases::new(
                &raw_snap_dir,
                &raw_local_dir,
                pwd.path_buf.as_path(),
                &alias_values,
            )?)
        } else {
            None
        };

        // don't want to request alt replicated mounts in snap mode
        let snaps_selected_for_search =
            if opt_alt_replicated && !matches!(exec_mode, ExecMode::SnapFileMount(_)) {
                SnapsSelectedForSearch::IncludeAltReplicated
            } else {
                SnapsSelectedForSearch::MostProximateOnly
            };

        Ok(FilesystemInfo {
            map_of_datasets: base_fs_info.map_of_datasets,
            map_of_snaps: base_fs_info.map_of_snaps,
            filter_dirs: base_fs_info.filter_dirs,
            opt_map_of_alts,
            opt_common_snap_dir,
            opt_map_of_aliases,
            snaps_selected_for_search,
        })
    }
}

// if we have some btrfs mounts, we check to see if there is a snap directory in common
// so we can hide that common path from searches later
pub fn get_common_snap_dir(
    map_of_datasets: &MapOfDatasets,
    map_of_snaps: &MapOfSnaps,
) -> Option<PathBuf> {
    let btrfs_datasets: Vec<&PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, dataset_info)| dataset_info.fs_type == FilesystemType::Btrfs)
        .map(|(mount, _dataset_info)| mount)
        .collect();

    if btrfs_datasets.is_empty() {
        // since snapshots ZFS reside on multiple datasets
        // never have a common snap path
        None
    } else {
        let vec_snaps: Vec<&PathBuf> = btrfs_datasets
            .into_par_iter()
            .filter_map(|mount| map_of_snaps.get(mount))
            .flat_map(|snap_info| snap_info)
            .collect();

        get_common_path(vec_snaps)
    }
}
