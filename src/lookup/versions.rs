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

use crate::config::generate::{Config, DedupBy, ExecMode, LastSnapMode};
use crate::data::paths::{CompareContentsContainer, PathData, PathDeconstruction};
use crate::filesystem::mounts::LinkType;
use crate::filesystem::snaps::MapOfSnaps;
use crate::interactive::preheat_cache::PreheatCache;
use crate::library::results::{HttmError, HttmResult};
use crate::{GLOBAL_CONFIG, MAP_OF_SNAPS};
use hashbrown::HashMap;
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;
use rayon::slice::ParallelSlice;
use std::borrow::Cow;
use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionsMap {
    inner: HashMap<PathData, Vec<PathData>>,
}

impl From<HashMap<PathData, Vec<PathData>>> for VersionsMap {
    fn from(map: HashMap<PathData, Vec<PathData>>) -> Self {
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
    type Target = HashMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for VersionsMap {
    fn deref_mut(&mut self) -> &mut HashMap<PathData, Vec<PathData>> {
        &mut self.inner
    }
}

impl VersionsMap {
    pub fn new(config: &Config, path_set: &[PathData]) -> HttmResult<VersionsMap> {
        let versions_map: VersionsMap = if path_set.len() == 1 {
            Self::from_one_path(config, &path_set[0])
                .into_iter()
                .map(|v| v.into_inner())
                .collect::<HashMap<PathData, Vec<PathData>>>()
                .into()
        } else {
            Self::from_many_paths(config, path_set).into()
        };

        // check if all files (snap and live) do not exist, if this is true, then user probably messed up
        // and entered a file that never existed (that is, perhaps a wrong file name)?
        if versions_map.values().all(std::vec::Vec::is_empty)
            && versions_map
                .keys()
                .all(|path_data| path_data.opt_path_metadata().is_none())
        {
            let paths: Vec<&Path> = path_set.iter().map(|path_data| path_data.path()).collect();

            let description = format!(
                "Requested paths do not currently exist, and httm could not find any deleted versions in snapshots:\n{:?}\nSo, umm, 🤷, please check that you typed the correct path, and/or try again with a different path.",
                paths
            );

            return HttmError::from(description).into();
        }

        Ok(versions_map)
    }

    #[inline(always)]
    fn from_many_paths(config: &Config, path_set: &[PathData]) -> HashMap<PathData, Vec<PathData>> {
        path_set
            .par_iter()
            .flat_map(|path_data| Self::from_one_path(config, path_data))
            .map(|versions| versions.into_inner())
            .collect()
    }

    #[inline(always)]
    fn from_one_path(config: &Config, path_data: &PathData) -> Option<Versions> {
        match Versions::new(config, path_data) {
            Ok(versions) => Some(versions),
            Err(err) => {
                if !matches!(config.exec_mode, ExecMode::Interactive(_)) {
                    eprintln!("WARN: {}", err.to_string())
                }

                None
            }
        }
        .map(|mut versions| {
            if config.opt_omit_ditto {
                versions.omit_ditto();
            }

            versions
        })
        .map(|mut versions| {
            if let Some(last_snap_mode) = &config.opt_last_snap {
                versions.last_snap(last_snap_mode);
            }

            versions
        })
    }
}

pub struct Versions {
    path_data_key: PathData,
    snap_versions: Vec<PathData>,
}

impl Versions {
    #[inline(always)]
    pub fn new(config: &Config, path_data: &PathData) -> HttmResult<Self> {
        let prox_opt_alts = ProximateDatasetAndOptAlts::new(config, path_data)?;
        let path_data_key = prox_opt_alts.path_data.clone();
        let snap_versions: Vec<PathData> = prox_opt_alts
            .into_search_bundles()
            .flat_map(|relative_path_snap_mounts| {
                relative_path_snap_mounts.version_search(&config.dedup_by)
            })
            .collect();

        Ok(Self {
            path_data_key,
            snap_versions,
        })
    }

    pub fn live_path_data(&self) -> &PathData {
        &self.path_data_key
    }

    pub fn snap_versions(&self) -> &[PathData] {
        &self.snap_versions
    }

    pub fn from_raw(path_data_key: PathData, snap_versions: Vec<PathData>) -> Self {
        Self {
            path_data_key,
            snap_versions,
        }
    }

    #[inline(always)]
    pub fn into_inner(self) -> (PathData, Vec<PathData>) {
        (self.path_data_key, self.snap_versions)
    }

    #[inline(always)]
    pub fn is_live_version_redundant(&self) -> bool {
        if let Some(last_snap) = self.snap_versions.last() {
            return last_snap.metadata_infallible() == self.path_data_key.metadata_infallible();
        }

        false
    }

    #[inline(always)]
    fn omit_ditto(&mut self) {
        if self.is_live_version_redundant() {
            self.snap_versions.pop();
        }
    }

    #[inline(always)]
    fn last_snap(&mut self, last_snap_mode: &LastSnapMode) {
        self.snap_versions = match self.snap_versions.last() {
            // if last() is some, then should be able to unwrap pop()
            Some(last) => match last_snap_mode {
                LastSnapMode::Any => vec![last.to_owned()],
                LastSnapMode::DittoOnly
                    if self.path_data_key.opt_path_metadata() == last.opt_path_metadata() =>
                {
                    vec![last.to_owned()]
                }
                LastSnapMode::NoDittoExclusive | LastSnapMode::NoDittoInclusive
                    if self.path_data_key.opt_path_metadata() != last.opt_path_metadata() =>
                {
                    vec![last.to_owned()]
                }
                _ => Vec::new(),
            },
            None => match last_snap_mode {
                LastSnapMode::Without | LastSnapMode::NoDittoInclusive => {
                    vec![self.path_data_key.clone()]
                }
                _ => Vec::new(),
            },
        };
    }
}

#[derive(Debug, Clone)]
pub struct ProximateDatasetAndOptAlts<'a> {
    config: &'a Config,
    path_data: &'a PathData,
    proximate_dataset: &'a Path,
    relative_path: &'a Path,
    opt_alts: Option<&'a [Box<Path>]>,
}

impl<'a> ProximateDatasetAndOptAlts<'a> {
    #[inline(always)]
    pub fn new(config: &'a Config, path_data: &'a PathData) -> HttmResult<Self> {
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
        let (proximate_dataset, relative_path) = path_data
            .alias()
            .map(|alias| (alias.proximate_dataset(), alias.relative_path()))
            .map_or_else(
                || {
                    path_data.proximate_dataset().and_then(|proximate_dataset| {
                        path_data
                            .relative_path(proximate_dataset)
                            .map(|relative_path| (proximate_dataset, relative_path))
                    })
                },
                Ok,
            )?;

        let opt_alts = config
            .dataset_collection
            .opt_map_of_alts
            .as_ref()
            .and_then(|map_of_alts| map_of_alts.get(proximate_dataset))
            .and_then(|alt_metadata| alt_metadata.deref().as_deref());

        Ok(Self {
            config,
            path_data,
            proximate_dataset,
            relative_path,
            opt_alts,
        })
    }

    #[inline(always)]
    pub fn path_data(&self) -> &PathData {
        &self.path_data
    }

    #[inline(always)]
    pub fn proximate_dataset(&self) -> &Path {
        &self.proximate_dataset
    }

    #[inline(always)]
    pub fn datasets_of_interest(&'a self) -> impl Iterator<Item = &'a Path> {
        let alts = self.opt_alts.into_iter().flatten().map(|p| p.as_ref());

        let base = Some(self.proximate_dataset).into_iter();

        alts.chain(base)
    }

    #[inline(always)]
    pub fn into_search_bundles(&'a self) -> impl Iterator<Item = RelativePathAndSnapMounts<'a>> {
        self.datasets_of_interest().flat_map(|dataset_of_interest| {
            RelativePathAndSnapMounts::new(
                self.config,
                self.path_data,
                &self.relative_path,
                &dataset_of_interest,
            )
        })
    }
}

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts<'a> {
    config: &'a Config,
    path_data: &'a PathData,
    relative_path: &'a Path,
    dataset_of_interest: &'a Path,
    snap_mounts: Cow<'a, [Box<Path>]>,
}

impl<'a> RelativePathAndSnapMounts<'a> {
    #[inline(always)]
    pub fn new(
        config: &'a Config,
        path_data: &'a PathData,
        relative_path: &'a Path,
        dataset_of_interest: &'a Path,
    ) -> Option<Self> {
        // building our relative path, by removing parent below the snap dir
        //
        // for native searches the prefix is are the dirs below the most proximate dataset
        // for user specified dirs/aliases these are specified by the user
        Self::snap_mounts_from_dataset_of_interest(dataset_of_interest, config).map(|snap_mounts| {
            Self {
                config,
                path_data,
                relative_path,
                dataset_of_interest,
                snap_mounts,
            }
        })
    }

    pub fn snap_mounts_from_dataset_of_interest(
        dataset_of_interest: &Path,
        config: &Config,
    ) -> Option<Cow<'a, [Box<Path>]>> {
        if !config.opt_debug && config.opt_lazy {
            // now process snaps
            return config
                .dataset_collection
                .map_of_datasets
                .get(dataset_of_interest)
                .map(|md| {
                    MapOfSnaps::from_defined_mounts(&dataset_of_interest, md, config.opt_debug)
                })
                .map(|snap_mounts| Cow::Owned(snap_mounts));
        }

        MAP_OF_SNAPS
            .get(dataset_of_interest)
            .map(|snap_mounts| Cow::Borrowed(snap_mounts.as_slice()))
    }

    #[inline(always)]
    pub fn snap_mounts(&'a self) -> &'a [Box<Path>] {
        &self.snap_mounts
    }

    #[inline(always)]
    pub fn relative_path(&'a self) -> &'a Path {
        &self.relative_path
    }

    #[inline(always)]
    pub fn dataset_of_interest(&'a self) -> &'a Path {
        &self.dataset_of_interest
    }

    #[inline(always)]
    pub fn config(&'a self) -> &'a Config {
        &self.config
    }

    #[inline(always)]
    pub fn path_data(&'a self) -> &'a PathData {
        &self.path_data
    }

    #[inline(always)]
    fn match_metadata(&self, joined_path: PathBuf) -> Option<PathData> {
        match joined_path.symlink_metadata() {
            Ok(md) => {
                // why not PathData::new()? because symlinks will resolve!
                // symlinks from a snap will end up looking just like the link target, so this is very confusing...
                Some(PathData::without_styling(&joined_path, Some(md)))
            }
            Err(err) => {
                match err.kind() {
                    // if we do not have permissions to read the snapshot directories
                    // fail/panic printing a descriptive error instead of flattening
                    ErrorKind::PermissionDenied => {
                        eprintln!(
                            "Error: When httm tried to find a file contained within a snapshot directory, permission was denied.  \
                        Perhaps you need to use sudo or equivalent to view the contents of this snapshot (for instance, btrfs by default creates privileged snapshots).  \
                        \nDetails: {err}"
                        );
                        std::process::exit(1)
                    }
                    // if file metadata is not found, or is otherwise not available,
                    // continue, it simply means we do not have a snapshot of this file
                    _ => None,
                }
            }
        }
    }

    #[inline(always)]
    fn all_versions(&'a self) -> Vec<PathData> {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        const CHUNK_SIZE: usize = 64;
        let snap_mounts_len = self.snap_mounts.len();

        if snap_mounts_len > CHUNK_SIZE {
            return self
                .snap_mounts
                .par_chunks(CHUNK_SIZE)
                .map(|chunk| {
                    chunk
                        .into_iter()
                        .map(|snap_path| snap_path.join(self.relative_path))
                        .filter_map(|joined_path| self.match_metadata(joined_path))
                })
                .flatten_iter()
                .collect::<Vec<PathData>>();
        }

        self.snap_mounts
            .into_iter()
            .map(|snap_path| snap_path.join(self.relative_path))
            .filter_map(|joined_path| self.match_metadata(joined_path))
            .collect::<Vec<PathData>>()
    }

    #[inline(always)]
    fn sort_dedup_versions(vec: &mut Vec<PathData>, dedup_by: &DedupBy) {
        match dedup_by {
            DedupBy::Disable => {
                vec.sort_unstable_by_key(|path_data| path_data.metadata_infallible());
            }
            DedupBy::Metadata => {
                vec.sort_unstable_by_key(|path_data| path_data.metadata_infallible());
                vec.dedup_by_key(|a| a.metadata_infallible());
            }
            DedupBy::Contents | DedupBy::Suspect => {
                let mut container_vec: Vec<CompareContentsContainer> = std::mem::take(vec)
                    .into_iter()
                    .map(|path_data| CompareContentsContainer::from(path_data))
                    .collect();

                container_vec.sort_unstable();
                container_vec.dedup();
                container_vec.sort_unstable_by_key(|path_data| path_data.metadata_infallible());

                *vec = container_vec
                    .into_iter()
                    .map(|container| container.into())
                    .collect()
            }
        }
    }

    #[inline(always)]
    pub fn version_search(&'a self, dedup_by: &DedupBy) -> Vec<PathData> {
        self.enable_preheat_cache();

        let mut versions = self.all_versions();

        Self::sort_dedup_versions(&mut versions, dedup_by);

        versions
    }

    fn enable_preheat_cache(&self) {
        if matches!(self.config().exec_mode, ExecMode::Preview)
            || self
                .config()
                .dataset_collection
                .map_of_datasets
                .get(self.dataset_of_interest())
                .is_some_and(|md| matches!(md.link_type, LinkType::Network))
        {
            let cache = GLOBAL_CONFIG
                .opt_preheat_cache
                .get_or_init(|| PreheatCache::new());
            cache.exec(self);
        }
    }
}
