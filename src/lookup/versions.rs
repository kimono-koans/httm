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

use std::{
    collections::BTreeMap,
    ops::Deref,
    path::{Path, PathBuf},
    time::SystemTime,
};

use rayon::prelude::*;

use crate::config::generate::{Config, LastSnapMode};
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionsMap {
    inner: BTreeMap<PathData, Vec<PathData>>,
}

impl From<BTreeMap<PathData, Vec<PathData>>> for VersionsMap {
    fn from(map: BTreeMap<PathData, Vec<PathData>>) -> Self {
        Self { inner: map }
    }
}

impl From<(PathData, Vec<PathData>)> for VersionsMap {
    fn from(tuple: (PathData, Vec<PathData>)) -> Self {
        Self {
            inner: BTreeMap::from([tuple]),
        }
    }
}

impl Deref for VersionsMap {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl VersionsMap {
    pub fn new(config: &Config, path_set: &[PathData]) -> HttmResult<VersionsMap> {
        let display_map = Self::from(config, path_set);

        // check if all files (snap and live) do not exist, if this is true, then user probably messed up
        // and entered a file that never existed (that is, perhaps a wrong file name)?
        if display_map.values().all(std::vec::Vec::is_empty)
            && display_map
                .keys()
                .all(|pathdata| pathdata.metadata.is_none())
            && !config.opt_no_snap
        {
            return Err(HttmError::new(
                "httm could not find either a live copy or a snapshot copy of any specified file, so, umm, 🤷? Please try another file.",
            )
            .into());
        }

        Ok(display_map)
    }

    pub fn from(config: &Config, path_set: &[PathData]) -> Self {
        // create vec of all local and replicated backups at once
        let snaps_selected_for_search = config
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let all_snap_versions: BTreeMap<PathData, Vec<PathData>> = path_set
            .iter()
            .map(|pathdata| {
                let snaps: Vec<PathData> = snaps_selected_for_search
                    .par_iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                    })
                    .flat_map(|dataset_for_search| {
                        dataset_for_search.get_search_bundles(config, pathdata)
                    })
                    .flatten()
                    .flat_map(|search_bundle| search_bundle.get_versions())
                    .filter(|snap_version| {
                        // process omit_ditto before last snap
                        if config.opt_omit_ditto {
                            snap_version.get_md_infallible() != pathdata.get_md_infallible()
                        } else {
                            true
                        }
                    })
                    .collect();
                (pathdata.clone(), snaps)
            })
            .collect();

        let display_map: VersionsMap = all_snap_versions.into();

        // process last snap mode after omit_ditto
        match &config.opt_last_snap {
            Some(last_snap_mode) => display_map.get_last_snap(last_snap_mode),
            None => display_map,
        }
    }

    fn get_last_snap(&self, last_snap_mode: &LastSnapMode) -> VersionsMap {
        let res: BTreeMap<PathData, Vec<PathData>> = self
            .iter()
            .map(|(pathdata, snaps)| {
                let new_snaps = match snaps.last() {
                    Some(last) => match last_snap_mode {
                        LastSnapMode::Any => vec![last.clone()],
                        LastSnapMode::DittoOnly
                            if pathdata.get_md_infallible() == last.get_md_infallible() =>
                        {
                            vec![last.clone()]
                        }
                        LastSnapMode::NoDittoExclusive
                            if pathdata.get_md_infallible() != last.get_md_infallible() =>
                        {
                            vec![last.clone()]
                        }
                        LastSnapMode::NoDittoInclusive
                            if pathdata.get_md_infallible() != last.get_md_infallible() =>
                        {
                            vec![last.clone()]
                        }
                        _ => Vec::new(),
                    },
                    None => match last_snap_mode {
                        LastSnapMode::Without | LastSnapMode::NoDittoInclusive => {
                            vec![pathdata.clone()]
                        }
                        _ => Vec::new(),
                    },
                };
                (pathdata.clone(), new_snaps)
            })
            .collect();

        res.into()
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

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MostProximateAndOptAlts {
    pub proximate_dataset_mount: PathBuf,
    pub opt_datasets_of_interest: Option<Vec<PathBuf>>,
}

impl MostProximateAndOptAlts {
    pub fn new(
        config: &Config,
        pathdata: &PathData,
        requested_dataset_type: &SnapDatasetType,
    ) -> HttmResult<Self> {
        // here, we take our file path and get back possibly multiple ZFS dataset mountpoints
        // and our most proximate dataset mount point (which is always the same) for
        // a single file
        //
        // we ask a few questions: has the location been user defined? if not, does
        // the user want all local datasets on the system, including replicated datasets?
        // the most common case is: just use the most proximate dataset mount point as both
        // the dataset of interest and most proximate ZFS dataset
        //
        // why? we need both the dataset of interest and the most proximate dataset because we
        // will compare the most proximate dataset to our our canonical path and the difference
        // between ZFS mount point and the canonical path is the path we will use to search the
        // hidden snapshot dirs
        let proximate_dataset_mount = match &config.dataset_collection.opt_map_of_aliases {
            Some(map_of_aliases) => match pathdata.get_alias_dataset(map_of_aliases) {
                Some(alias_snap_dir) => alias_snap_dir,
                None => {
                    pathdata.get_proximate_dataset(&config.dataset_collection.map_of_datasets)?
                }
            },
            None => pathdata.get_proximate_dataset(&config.dataset_collection.map_of_datasets)?,
        };

        let snap_types_for_search: MostProximateAndOptAlts = match requested_dataset_type {
            SnapDatasetType::MostProximate => {
                // just return the same dataset when in most proximate mode
                Self {
                    proximate_dataset_mount,
                    opt_datasets_of_interest: None,
                }
            }
            SnapDatasetType::AltReplicated => match &config.dataset_collection.opt_map_of_alts {
                Some(map_of_alts) => match map_of_alts.get(proximate_dataset_mount.as_path()) {
                    Some(snap_types_for_search) => snap_types_for_search.clone(),
                    None => return Err(HttmError::new("If you are here a map of alts is missing for a supplied mount, \
                    this is fine as we should just flatten/ignore this error.").into()),
                },
                None => unreachable!("If config option alt-replicated is specified, then a map of alts should have been generated, \
                if you are here such a map is missing."),
            },
        };

        Ok(snap_types_for_search)
    }

    pub fn get_search_bundles(
        &self,
        config: &Config,
        pathdata: &PathData,
    ) -> HttmResult<Vec<RelativePathAndSnapMounts>> {
        let proximate_dataset_mount = self.proximate_dataset_mount.as_path();

        match &self.opt_datasets_of_interest {
            Some(datasets_of_interest) => datasets_of_interest
                .iter()
                .map(|dataset_of_interest| {
                    RelativePathAndSnapMounts::new(
                        config,
                        pathdata,
                        proximate_dataset_mount,
                        dataset_of_interest,
                    )
                })
                .collect(),
            None => Ok(vec![RelativePathAndSnapMounts::new(
                config,
                pathdata,
                proximate_dataset_mount,
                proximate_dataset_mount,
            )?]),
        }
    }

    pub fn get_datasets_of_interest(self) -> Vec<PathBuf> {
        self.opt_datasets_of_interest
            .unwrap_or_else(|| vec![self.proximate_dataset_mount])
    }
}

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts {
    pub relative_path: PathBuf,
    pub snap_mounts: Vec<PathBuf>,
}

impl RelativePathAndSnapMounts {
    fn new(
        config: &Config,
        pathdata: &PathData,
        proximate_dataset_mount: &Path,
        dataset_of_interest: &Path,
    ) -> HttmResult<Self> {
        // building our relative path by removing parent below the snap dir
        //
        // for native searches the prefix is are the dirs below the most proximate dataset
        // for user specified dirs/aliases these are specified by the user
        let relative_path = pathdata.get_relative_path(config, proximate_dataset_mount)?;

        let snap_mounts = config
            .dataset_collection
            .map_of_snaps
            .get(dataset_of_interest)
            .ok_or_else(|| {
                HttmError::new(
                    "httm could find no snap mount for your files.  \
                Iterator should just ignore/flatten this error.",
                )
            })
            .cloned()?;

        Ok(Self {
            relative_path,
            snap_mounts,
        })
    }

    fn get_versions(&self) -> Vec<PathData> {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        //
        // BTreeMap will then remove duplicates with the same system modify time and size/file len
        let unique_versions: BTreeMap<(SystemTime, u64), PathData> = self
            .snap_mounts
            .par_iter()
            .map(|path| path.join(&self.relative_path))
            .map(|joined_path| PathData::from(joined_path.as_path()))
            .filter_map(|pathdata| {
                pathdata
                    .metadata
                    .map(|metadata| ((metadata.modify_time, metadata.size), pathdata))
            })
            .collect();

        let sorted_versions: Vec<PathData> = unique_versions.into_values().collect();

        sorted_versions
    }
}
