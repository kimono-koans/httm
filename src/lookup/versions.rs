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

use hashbrown::HashSet;
use rayon::prelude::*;

use crate::config::generate::{Config, DedupBy, ExecMode, LastSnapMode};
use crate::data::paths::PathDeconstruction;
use crate::data::paths::{CompareVersionsContainer, PathData};
use crate::library::results::{HttmError, HttmResult};
use crate::parse::mounts::LinkType;
use crate::GLOBAL_CONFIG;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionsMap {
    inner: BTreeMap<PathData, Vec<PathData>>,
}

impl From<BTreeMap<PathData, Vec<PathData>>> for VersionsMap {
    fn from(map: BTreeMap<PathData, Vec<PathData>>) -> Self {
        Self { inner: map }
    }
}

impl From<[(PathData, Vec<PathData>); 1]> for VersionsMap {
    fn from(slice: [(PathData, Vec<PathData>); 1]) -> Self {
        Self {
            inner: slice.into(),
        }
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
    pub fn new(config: &Config, path_set: &[PathData]) -> HttmResult<VersionsMap> {
        let is_interactive_mode = matches!(GLOBAL_CONFIG.exec_mode, ExecMode::Interactive(_));

        let all_snap_versions: BTreeMap<PathData, Vec<PathData>> = path_set
            .par_iter()
            .filter_map(|pathdata| match Versions::new(pathdata, config) {
                Ok(versions) => Some(versions),
                Err(err) => {
                    if !is_interactive_mode {
                        eprintln!("WARN: {}", err.to_string())
                    }
                    None
                }
            })
            .map(|versions| {
                if !is_interactive_mode
                    && versions.live_path.opt_metadata().is_none()
                    && versions.snap_versions.is_empty()
                {
                    eprintln!(
                        "WARN: Input file may have never existed: {:?}",
                        versions.live_path.path()
                    );
                }

                versions.into_inner()
            })
            .collect();

        let mut versions_map: VersionsMap = all_snap_versions.into();

        // check if all files (snap and live) do not exist, if this is true, then user probably messed up
        // and entered a file that never existed (that is, perhaps a wrong file name)?
        if versions_map.values().all(std::vec::Vec::is_empty)
            && versions_map
                .keys()
                .all(|pathdata| pathdata.opt_metadata().is_none())
        {
            return Err(HttmError::new(
                "httm could find neither a live version, nor any snapshot version for all the specified paths, so, umm, 🤷? Please try another file.",
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
            return last_snap.opt_metadata() == live_pathdata.opt_metadata();
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
                    LastSnapMode::DittoOnly if pathdata.opt_metadata() == last.opt_metadata() => {
                        vec![last.to_owned()]
                    }
                    LastSnapMode::NoDittoExclusive | LastSnapMode::NoDittoInclusive
                        if pathdata.opt_metadata() != last.opt_metadata() =>
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

pub struct Versions {
    live_path: PathData,
    snap_versions: Vec<PathData>,
}

impl Versions {
    #[inline(always)]
    pub fn new(pathdata: &PathData, config: &Config) -> HttmResult<Self> {
        let prox_opt_alts = ProximateDatasetAndOptAlts::new(pathdata)?;
        let live_path = prox_opt_alts.pathdata.clone();
        let snap_versions: Vec<PathData> = prox_opt_alts
            .into_search_bundles()
            .flat_map(|relative_path_snap_mounts| {
                relative_path_snap_mounts.versions_processed(&config.dedup_by)
            })
            .collect();

        Ok(Self {
            live_path,
            snap_versions,
        })
    }

    #[inline(always)]
    pub fn into_inner(self) -> (PathData, Vec<PathData>) {
        (self.live_path, self.snap_versions)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ProximateDatasetAndOptAlts<'a> {
    pub pathdata: &'a PathData,
    pub proximate_dataset: &'a Path,
    pub relative_path: &'a Path,
    pub opt_alts: Option<&'a Vec<PathBuf>>,
}

impl<'a> Ord for ProximateDatasetAndOptAlts<'a> {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.pathdata.cmp(&other.pathdata)
    }
}

impl<'a> PartialOrd for ProximateDatasetAndOptAlts<'a> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> ProximateDatasetAndOptAlts<'a> {
    #[inline(always)]
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
        let (proximate_dataset, relative_path) = pathdata
            .alias()
            .map(|alias| (alias.proximate_dataset, alias.relative_path))
            .map_or_else(
                || {
                    pathdata.proximate_dataset().and_then(|proximate_dataset| {
                        pathdata
                            .relative_path(proximate_dataset)
                            .map(|relative_path| (proximate_dataset, relative_path))
                    })
                },
                Ok,
            )?;

        let opt_alts = GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_alts
            .as_ref()
            .and_then(|map_of_alts| map_of_alts.get(proximate_dataset))
            .and_then(|alt_metadata| alt_metadata.opt_datasets_of_interest.as_ref());

        Ok(Self {
            pathdata,
            proximate_dataset,
            relative_path,
            opt_alts,
        })
    }
    #[inline(always)]
    pub fn datasets_of_interest(&'a self) -> impl Iterator<Item = &'a Path> {
        let alts = self.opt_alts.into_iter().flatten().map(PathBuf::as_path);

        let base = Some(self.proximate_dataset).into_iter();

        alts.chain(base)
    }
    #[inline(always)]
    pub fn into_search_bundles(&'a self) -> impl Iterator<Item = RelativePathAndSnapMounts<'a>> {
        self.datasets_of_interest().flat_map(|dataset_of_interest| {
            RelativePathAndSnapMounts::new(&self.relative_path, &dataset_of_interest)
        })
    }
}

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts<'a> {
    pub relative_path: &'a Path,
    pub snap_mounts: &'a [PathBuf],
    pub dataset_of_interest: &'a Path,
}

impl<'a> RelativePathAndSnapMounts<'a> {
    #[inline(always)]
    fn new(relative_path: &'a Path, dataset_of_interest: &'a Path) -> Option<Self> {
        // building our relative path by removing parent below the snap dir
        //
        // for native searches the prefix is are the dirs below the most proximate dataset
        // for user specified dirs/aliases these are specified by the user
        GLOBAL_CONFIG
            .dataset_collection
            .map_of_snaps
            .get(dataset_of_interest)
            .map(|snap_mounts| Self {
                relative_path,
                snap_mounts,
                dataset_of_interest,
            })
    }
    #[inline(always)]
    pub fn versions_processed(&'a self, dedup_by: &DedupBy) -> Vec<PathData> {
        loop {
            let all_versions = self.all_versions_unprocessed();

            let res = Self::sort_dedup_versions(all_versions, dedup_by);

            if res.is_empty() {
                // opendir and readdir iter on the snap path are necessary to mount snapshots over SMB
                match NetworkAutoMount::new(&self) {
                    NetworkAutoMount::Break => break res,
                    NetworkAutoMount::Continue => continue,
                }
            }

            break res;
        }
    }

    pub fn last_version(&self) -> Option<PathData> {
        let mut sorted_versions = self.versions_processed(&DedupBy::Disable);

        sorted_versions.pop()
    }

    #[inline(always)]
    fn all_versions_unprocessed(&'a self) -> impl Iterator<Item = PathData> + 'a {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        self
            .snap_mounts
            .iter()
            .map(|snap_path| {
                snap_path.join(self.relative_path)
            })
            .filter_map(|joined_path| {
                match joined_path.symlink_metadata() {
                    Ok(md) => {
                        // why not PathData::new()? because symlinks will resolve!
                        // symlinks from a snap will end up looking just like the link target, so this is very confusing...
                        Some(PathData::new(&joined_path, Some(md)))
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

    // remove duplicates with the same system modify time and size/file len (or contents! See --DEDUP_BY)
    #[inline(always)]
    fn sort_dedup_versions(
        iter: impl Iterator<Item = PathData>,
        dedup_by: &DedupBy,
    ) -> Vec<PathData> {
        match dedup_by {
            DedupBy::Disable => {
                let mut vec: Vec<PathData> = iter.collect();
                vec.sort_unstable_by_key(|pathdata| pathdata.metadata_infallible().mtime());
                vec
            }
            DedupBy::Contents | DedupBy::Metadata => {
                let mut vec: Vec<CompareVersionsContainer> = iter
                    .into_iter()
                    .map(|pathdata| CompareVersionsContainer::new(pathdata, dedup_by))
                    .collect();

                vec.sort_unstable();
                vec.dedup_by(|a, b| a.cmp(&b) == Ordering::Equal);

                vec.into_iter().map(|container| container.into()).collect()
            }
        }
    }
}

enum NetworkAutoMount {
    Break,
    Continue,
}

impl NetworkAutoMount {
    #[inline(always)]
    fn new(bundle: &RelativePathAndSnapMounts) -> NetworkAutoMount {
        if GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(bundle.dataset_of_interest)
            .map(|md| matches!(md.link_type, LinkType::Local))
            .unwrap_or_else(|| true)
        {
            return NetworkAutoMount::Break;
        }

        static CACHE_RESULT: LazyLock<RwLock<HashSet<PathBuf>>> =
            LazyLock::new(|| RwLock::new(HashSet::new()));

        if CACHE_RESULT
            .try_read()
            .ok()
            .map(|cached_result| cached_result.contains(bundle.dataset_of_interest))
            .unwrap_or_else(|| true)
        {
            return NetworkAutoMount::Break;
        }

        if let Ok(mut cached_result) = CACHE_RESULT.try_write() {
            cached_result.insert_unique_unchecked(bundle.dataset_of_interest.to_path_buf());

            bundle.snap_mounts.iter().for_each(|snap_path| {
                let _ = std::fs::read_dir(snap_path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .next();
            });

            return NetworkAutoMount::Continue;
        }

        NetworkAutoMount::Break
    }
}
