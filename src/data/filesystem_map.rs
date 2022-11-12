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

use crate::data::paths::PathData;
use crate::lookup::versions::MostProximateAndOptAlts;
use std::{collections::BTreeMap, path::PathBuf};

pub type MapOfDatasets = BTreeMap<PathBuf, DatasetMetadata>;
pub type MapOfSnaps = BTreeMap<PathBuf, Vec<PathBuf>>;
pub type MapOfAlts = BTreeMap<PathBuf, MostProximateAndOptAlts>;
pub type MapOfAliases = BTreeMap<PathBuf, RemotePathAndFsType>;
pub type MapLiveToSnaps = BTreeMap<PathData, Vec<PathData>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilesystemType {
    Zfs,
    Btrfs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MountType {
    Local,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePathAndFsType {
    pub remote_dir: PathBuf,
    pub fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetMetadata {
    pub name: String,
    pub fs_type: FilesystemType,
    pub mount_type: MountType,
}

#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum SnapDatasetType {
    MostProximate,
    AltReplicated,
}

#[derive(Copy, Debug, Clone, PartialEq, Eq)]
pub enum SnapsSelectedForSearch {
    MostProximateOnly,
    IncludeAltReplicated,
}

// alt replicated should come first,
// so as to be at the top of results
static INCLUDE_ALTS: &[SnapDatasetType] = [
    SnapDatasetType::AltReplicated,
    SnapDatasetType::MostProximate,
]
.as_slice();

static ONLY_PROXIMATE: &[SnapDatasetType] = [SnapDatasetType::MostProximate].as_slice();

impl SnapsSelectedForSearch {
    pub fn get_value(&self) -> &[SnapDatasetType] {
        match self {
            SnapsSelectedForSearch::IncludeAltReplicated => INCLUDE_ALTS,
            SnapsSelectedForSearch::MostProximateOnly => ONLY_PROXIMATE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetCollection {
    // key: mount, val: (dataset/subvol, fs_type, mount_type)
    pub map_of_datasets: MapOfDatasets,
    // key: mount, val: vec snap locations on disk (e.g. /.zfs/snapshot/snap_8a86e4fc_prepApt/home)
    pub map_of_snaps: MapOfSnaps,
    // key: mount, val: alt dataset
    pub opt_map_of_alts: Option<MapOfAlts>,
    // key: local dir, val: (remote dir, fstype)
    pub opt_map_of_aliases: Option<MapOfAliases>,
    // vec dirs to be filtered
    pub vec_of_filter_dirs: Vec<PathBuf>,
    // opt single dir to to be filtered re: btrfs common snap dir
    pub opt_common_snap_dir: Option<PathBuf>,
    // vec of two enum variants - most proximate and alt replicated, or just most proximate
    pub snaps_selected_for_search: SnapsSelectedForSearch,
}

use std::ffi::OsStr;

use clap::OsValues;

use crate::config::generate::ExecMode;
use crate::library::results::HttmResult;
use crate::parse::aliases::parse_aliases;
use crate::parse::alts::precompute_alt_replicated;
use crate::parse::mounts::{get_common_snap_dir, parse_mounts_exec};

impl DatasetCollection {
    pub fn new(
        opt_alt_replicated: bool,
        opt_remote_dir: Option<&OsStr>,
        opt_local_dir: Option<&OsStr>,
        opt_map_aliases: Option<OsValues>,
        pwd: &PathData,
        exec_mode: &ExecMode,
    ) -> HttmResult<DatasetCollection> {
        let (map_of_datasets, map_of_snaps, vec_of_filter_dirs) = parse_mounts_exec()?;

        // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
        let opt_common_snap_dir = get_common_snap_dir(&map_of_datasets, &map_of_snaps);

        // only create a map of alts if necessary
        let opt_map_of_alts = if opt_alt_replicated {
            Some(precompute_alt_replicated(&map_of_datasets))
        } else {
            None
        };

        let alias_values: Option<Vec<String>> =
            if let Some(env_map_aliases) = std::env::var_os("HTTM_MAP_ALIASES") {
                Some(
                    env_map_aliases
                        .to_string_lossy()
                        .split_terminator(',')
                        .map(|str| str.to_owned())
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

            Some(parse_aliases(
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

        Ok(DatasetCollection {
            map_of_datasets,
            map_of_snaps,
            opt_map_of_alts,
            vec_of_filter_dirs,
            opt_common_snap_dir,
            opt_map_of_aliases,
            snaps_selected_for_search,
        })
    }
}
