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
use std::{collections::BTreeMap, path::PathBuf};

pub type MapOfDatasets = BTreeMap<PathBuf, DatasetMetadata>;
pub type MapOfSnaps = BTreeMap<PathBuf, VecOfSnaps>;
pub type MapOfAlts = BTreeMap<PathBuf, MostProximateAndOptAlts>;
pub type MapOfAliases = BTreeMap<PathBuf, RemotePathAndFsType>;
pub type BtrfsCommonSnapDir = PathBuf;
pub type VecOfFilterDirs = Vec<PathBuf>;
pub type VecOfSnaps = Vec<PathBuf>;
pub type MapLiveToSnaps = BTreeMap<PathData, Vec<PathData>>;
pub type SnapsAndLiveSet = [Vec<PathData>; 2];
pub type OptMapOfAlts = Option<MapOfAlts>;
pub type OptMapOfAliases = Option<MapOfAliases>;
pub type OptBtrfsCommonSnapDir = Option<BtrfsCommonSnapDir>;

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

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MostProximateAndOptAlts {
    pub proximate_dataset_mount: PathBuf,
    pub opt_datasets_of_interest: Option<Vec<PathBuf>>,
}

impl MostProximateAndOptAlts {
    pub fn get_datasets_of_interest(self) -> Vec<PathBuf> {
        self.opt_datasets_of_interest
            .unwrap_or_else(|| vec![self.proximate_dataset_mount])
    }
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
    pub opt_map_of_alts: OptMapOfAlts,
    // key: local dir, val: (remote dir, fstype)
    pub opt_map_of_aliases: OptMapOfAliases,
    // vec dirs to be filtered
    pub vec_of_filter_dirs: VecOfFilterDirs,
    // opt single dir to to be filtered re: btrfs common snap dir
    pub opt_common_snap_dir: OptBtrfsCommonSnapDir,
    // vec of two enum variants - most proximate and alt replicated, or just most proximate
    pub snaps_selected_for_search: SnapsSelectedForSearch,
}
