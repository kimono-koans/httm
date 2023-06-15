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

use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    ops::Deref,
    ops::DerefMut,
    path::{Path, PathBuf},
};

use rayon::prelude::*;

use crate::library::results::{HttmError, HttmResult};
use crate::{
    config::generate::ListSnapsOfType,
    data::paths::{CompareVersionsContainer, PathData},
};
use crate::{
    config::generate::{BulkExclusion, Config, LastSnapMode},
    GLOBAL_CONFIG,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionsMap {
    inner: BTreeMap<PathData, Vec<PathData>>,
}

impl From<BTreeMap<PathData, Vec<PathData>>> for VersionsMap {
    fn from(map: BTreeMap<PathData, Vec<PathData>>) -> Self {
        Self { inner: map }
    }
}

impl Deref for VersionsMap {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for VersionsMap {
    fn deref_mut(&mut self) -> &mut BTreeMap<PathData, Vec<PathData>> {
        &mut self.inner
    }
}

impl VersionsMap {
    pub fn into_inner(self) -> BTreeMap<PathData, Vec<PathData>> {
        self.inner
    }

    pub fn new(config: &Config, path_set: &[PathData]) -> HttmResult<VersionsMap> {
        let all_snap_versions: BTreeMap<PathData, Vec<PathData>> = path_set
            .par_iter()
            .flat_map(ProximateDatasetAndOptAlts::new)
            .map(|prox_opt_alts| {
                // don't want to flatten this iter here b/c
                // we want to keep these values with this key
                let key = prox_opt_alts.pathdata.clone();
                let values = prox_opt_alts
                    .into_search_bundles()
                    .map(|relative_path_snap_mounts| {
                        relative_path_snap_mounts.versions_processed(&config.uniqueness)
                    })
                    .flatten()
                    .collect();
                (key, values)
            })
            .collect();

        let mut versions_map: VersionsMap = all_snap_versions.into();

        // check if all files (snap and live) do not exist, if this is true, then user probably messed up
        // and entered a file that never existed (that is, perhaps a wrong file name)?
        if versions_map.values().all(std::vec::Vec::is_empty)
            && versions_map
                .keys()
                .all(|pathdata| pathdata.metadata.is_none())
            && !matches!(config.opt_bulk_exclusion, Some(BulkExclusion::NoSnap))
        {
            return Err(HttmError::new(
                "httm could not find either a live copy or a snapshot copy of any specified file, so, umm, 🤷? Please try another file.",
            )
            .into());
        }

        // process last snap mode after omit_ditto
        if config.opt_omit_ditto {
            versions_map.omit_ditto()
        }

        if let Some(last_snap_mode) = &config.opt_last_snap {
            versions_map.last_snap(last_snap_mode)
        }

        Ok(versions_map)
    }

    pub fn is_live_version_redundant(live_pathdata: &PathData, snaps: &[PathData]) -> bool {
        if let Some(last_snap) = snaps.last() {
            return last_snap.md_infallible() == live_pathdata.md_infallible();
        }

        false
    }

    fn omit_ditto(&mut self) {
        self.iter_mut().for_each(|(pathdata, snaps)| {
            // process omit_ditto before last snap
            if Self::is_live_version_redundant(pathdata, snaps) {
                snaps.pop();
            }
        });
    }

    fn last_snap(&mut self, last_snap_mode: &LastSnapMode) {
        self.iter_mut().for_each(|(pathdata, snaps)| {
            *snaps = match snaps.last() {
                // if last() is some, then should be able to unwrap pop()
                Some(last) => match last_snap_mode {
                    LastSnapMode::Any => vec![last.to_owned()],
                    LastSnapMode::DittoOnly if pathdata.md_infallible() == last.md_infallible() => {
                        vec![last.to_owned()]
                    }
                    LastSnapMode::NoDittoExclusive | LastSnapMode::NoDittoInclusive
                        if pathdata.md_infallible() != last.md_infallible() =>
                    {
                        vec![last.to_owned()]
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
        });
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProximateDatasetAndOptAlts<'a> {
    pub pathdata: &'a PathData,
    pub proximate_dataset_mount: &'a Path,
    pub datasets_of_interest: Vec<PathBuf>,
}

impl<'a> ProximateDatasetAndOptAlts<'a> {
    pub fn new(pathdata: &'a PathData) -> HttmResult<Self> {
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
        let proximate_dataset_mount: &Path = match GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_aliases
            .as_ref()
            .and_then(|map_of_aliases| pathdata.alias_dataset(map_of_aliases))
        {
            Some(alias_dataset) => alias_dataset,
            None => {
                pathdata.proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)?
            }
        };

        let res: Self = match GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_alts
            .as_ref()
            .and_then(|map_of_alts| map_of_alts.get(proximate_dataset_mount))
            .and_then(|alt_metadata| alt_metadata.opt_datasets_of_interest.clone())
        {
            Some(mut datasets_of_interest) => {
                datasets_of_interest.push(proximate_dataset_mount.to_path_buf());

                Self {
                    pathdata,
                    proximate_dataset_mount,
                    datasets_of_interest,
                }
            }

            None => Self {
                pathdata,
                proximate_dataset_mount,
                datasets_of_interest: vec![proximate_dataset_mount.to_path_buf()],
            },
        };

        Ok(res)
    }

    pub fn into_search_bundles(self) -> impl Iterator<Item = RelativePathAndSnapMounts<'a>> {
        self.datasets_of_interest
            .into_iter()
            .flat_map(|dataset_of_interest| {
                RelativePathAndSnapMounts::new(
                    self.pathdata,
                    self.proximate_dataset_mount,
                    &dataset_of_interest,
                )
            })
    }
}

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts<'a> {
    pub pathdata: &'a PathData,
    pub relative_path: &'a Path,
    pub snap_mounts: &'a Vec<PathBuf>,
}

impl<'a> RelativePathAndSnapMounts<'a> {
    fn new(
        pathdata: &'a PathData,
        proximate_dataset_mount: &'a Path,
        dataset_of_interest: &Path,
    ) -> HttmResult<Self> {
        // building our relative path by removing parent below the snap dir
        //
        // for native searches the prefix is are the dirs below the most proximate dataset
        // for user specified dirs/aliases these are specified by the user
        let relative_path = pathdata.relative_path(proximate_dataset_mount)?;

        let snap_mounts = GLOBAL_CONFIG
            .dataset_collection
            .map_of_snaps
            .get(dataset_of_interest)
            .ok_or_else(|| {
                HttmError::new(
                    "httm could find no snap mount for your files.  \
                Iterator should just ignore/flatten this error.",
                )
            })?;

        Ok(Self {
            pathdata,
            relative_path,
            snap_mounts,
        })
    }

    pub fn versions_processed(&'a self, uniqueness: &ListSnapsOfType) -> Vec<PathData> {
        let all_versions = self.versions_unprocessed(uniqueness);

        Self::sort_dedup_versions(all_versions, uniqueness)
    }

    pub fn last_version(&self) -> Option<PathData> {
        let mut sorted_versions = self.versions_processed(&ListSnapsOfType::All);

        sorted_versions.pop()
    }

    fn versions_unprocessed(
        &'a self,
        uniqueness: &'a ListSnapsOfType,
    ) -> impl ParallelIterator<Item = CompareVersionsContainer> + 'a {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        self
            .snap_mounts
            .par_iter()
            .map(|path| path.join(self.relative_path))
            .filter_map(|joined_path| {
                match joined_path.symlink_metadata() {
                    Ok(md) => {
                        Some(CompareVersionsContainer::new(PathData::new(joined_path.as_path(), Some(md)), uniqueness))
                    },
                    Err(err) => {
                        match err.kind() {
                            // if we do not have permissions to read the snapshot directories
                            // fail/panic printing a descriptive error instead of flattening
                            ErrorKind::PermissionDenied => {
                                eprintln!("Error: When httm tried to find a file contained within a snapshot directory, permission was denied.  \
                                Perhaps you need to use sudo or equivalent to view the contents of this snapshot (for instance, btrfs by default creates privileged snapshots).  \
                                \nDetails: {err}");
                                std::process::exit(1)
                            },
                            // if file metadata is not found, or is otherwise not available, 
                            // continue, it simply means we do not have a snapshot of this file
                            _ => None,
                        }
                    },
                }
            })
    }

    // remove duplicates with the same system modify time and size/file len (or contents! See --uniqueness)
    #[allow(clippy::mutable_key_type)]
    fn sort_dedup_versions(
        iter: impl ParallelIterator<Item = CompareVersionsContainer>,
        snaps_of_type: &ListSnapsOfType,
    ) -> Vec<PathData> {
        match snaps_of_type {
            ListSnapsOfType::All => iter.map(PathData::from).collect(),
            ListSnapsOfType::UniqueContents | ListSnapsOfType::UniqueMetadata => {
                let sorted_and_deduped: BTreeSet<CompareVersionsContainer> = iter.collect();
                sorted_and_deduped.into_iter().map(PathData::from).collect()
            }
        }
    }
}
