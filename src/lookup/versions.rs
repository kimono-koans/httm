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
    cmp::{Ord, Ordering, PartialOrd},
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, ErrorKind},
    ops::Deref,
    ops::DerefMut,
    path::{Path, PathBuf},
};

use once_cell::sync::OnceCell;
use rayon::prelude::*;
use simd_adler32::Adler32;

use crate::data::paths::PathMetadata;
use crate::library::results::{HttmError, HttmResult};
use crate::{config::generate::ListSnapsOfType, data::paths::PathData};
use crate::{
    config::generate::{BulkExclusion, Config, LastSnapMode},
    GLOBAL_CONFIG,
};

//use super::common::FindVersions;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionsMap {
    pub inner: BTreeMap<PathData, Vec<PathData>>,
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

impl DerefMut for VersionsMap {
    fn deref_mut(&mut self) -> &mut BTreeMap<PathData, Vec<PathData>> {
        &mut self.inner
    }
}

impl VersionsMap {
    pub fn new(config: &Config, path_set: &[PathData]) -> HttmResult<VersionsMap> {
        let mut versions_map = Self::exec(config, path_set);

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
            versions_map.get_last_snap(last_snap_mode)
        }

        Ok(versions_map)
    }

    fn exec(config: &Config, path_set: &[PathData]) -> Self {
        // create vec of all local and replicated backups at once
        let snaps_selected_for_search = GLOBAL_CONFIG
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let all_snap_versions: BTreeMap<PathData, Vec<PathData>> = path_set
            .par_iter()
            .map(|pathdata| {
                let snaps: Vec<PathData> = snaps_selected_for_search
                    .iter()
                    .flat_map(|dataset_type| MostProximateAndOptAlts::new(pathdata, dataset_type))
                    .flat_map(|datasets_of_interest| {
                        MostProximateAndOptAlts::get_search_bundles(datasets_of_interest, pathdata)
                    })
                    .flatten()
                    .flat_map(|search_bundle| search_bundle.get_unique_versions(&config.uniqueness))
                    .collect();
                (pathdata.clone(), snaps)
            })
            .collect();

        let versions_map: VersionsMap = all_snap_versions.into();

        versions_map
    }

    fn omit_ditto(&mut self) {
        self.iter_mut().for_each(|(pathdata, snaps)| {
            // process omit_ditto before last snap
            if let Some(last_snap) = snaps.last() {
                if last_snap.get_md_infallible() == pathdata.get_md_infallible() {
                    snaps.pop();
                }
            }
        });
    }

    fn get_last_snap(&mut self, last_snap_mode: &LastSnapMode) {
        self.iter_mut().for_each(|(pathdata, snaps)| {
            *snaps = match snaps.last() {
                // if last() is some, then should be able to unwrap pop()
                Some(last) => match last_snap_mode {
                    LastSnapMode::Any => vec![snaps.pop().unwrap()],
                    LastSnapMode::DittoOnly
                        if pathdata.get_md_infallible() == last.get_md_infallible() =>
                    {
                        vec![snaps.pop().unwrap()]
                    }
                    LastSnapMode::NoDittoExclusive | LastSnapMode::NoDittoInclusive
                        if pathdata.get_md_infallible() != last.get_md_infallible() =>
                    {
                        vec![snaps.pop().unwrap()]
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
pub static INCLUDE_ALTS: &[SnapDatasetType] = [
    SnapDatasetType::AltReplicated,
    SnapDatasetType::MostProximate,
]
.as_slice();

pub static ONLY_PROXIMATE: &[SnapDatasetType] = [SnapDatasetType::MostProximate].as_slice();

impl SnapsSelectedForSearch {
    pub fn get_value(&self) -> &[SnapDatasetType] {
        match self {
            SnapsSelectedForSearch::IncludeAltReplicated => INCLUDE_ALTS,
            SnapsSelectedForSearch::MostProximateOnly => ONLY_PROXIMATE,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MostProximateAndOptAlts<'a> {
    pub proximate_dataset_mount: &'a Path,
    pub opt_datasets_of_interest: &'a Option<Vec<PathBuf>>,
}

impl<'a> MostProximateAndOptAlts<'a> {
    pub fn new(
        pathdata: &'a PathData,
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
        let proximate_dataset_mount = match &GLOBAL_CONFIG.dataset_collection.opt_map_of_aliases {
            Some(map_of_aliases) => match pathdata.get_alias_dataset(map_of_aliases) {
                Some(alias_snap_dir) => alias_snap_dir,
                None => pathdata
                    .get_proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)?,
            },
            None => {
                pathdata.get_proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)?
            }
        };

        let snap_types_for_search: MostProximateAndOptAlts = match requested_dataset_type {
            SnapDatasetType::MostProximate => {
                // just return the same dataset when in most proximate mode
                Self {
                    proximate_dataset_mount,
                    opt_datasets_of_interest: &None,
                }
            }
            SnapDatasetType::AltReplicated => match &GLOBAL_CONFIG.dataset_collection.opt_map_of_alts {
                Some(map_of_alts) => match map_of_alts.get(proximate_dataset_mount) {
                    Some(snap_types_for_search) => {
                        MostProximateAndOptAlts {
                            proximate_dataset_mount: &snap_types_for_search.proximate_dataset_mount,
                            opt_datasets_of_interest: &snap_types_for_search.opt_datasets_of_interest,
                        }
                    },
                    None => return Err(HttmError::new("If you are here a map of alts is missing for a supplied mount, \
                    this is fine as we should just flatten/ignore this error.").into()),
                },
                None => unreachable!("If config option alt-replicated is specified, then a map of alts should have been generated, \
                if you are here such a map is missing."),
            },
        };

        Ok(snap_types_for_search)
    }

    pub fn get_search_bundles<'b>(
        datasets_of_interest: MostProximateAndOptAlts<'b>,
        pathdata: &'b PathData,
    ) -> HttmResult<Vec<RelativePathAndSnapMounts<'b>>> {
        let proximate_dataset_mount = datasets_of_interest.proximate_dataset_mount;

        match datasets_of_interest.opt_datasets_of_interest {
            Some(datasets) => datasets
                .iter()
                .map(|dataset_of_interest| {
                    RelativePathAndSnapMounts::new(
                        pathdata,
                        proximate_dataset_mount,
                        dataset_of_interest,
                    )
                })
                .collect(),
            None => Ok(vec![RelativePathAndSnapMounts::new(
                pathdata,
                proximate_dataset_mount,
                proximate_dataset_mount,
            )?]),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts<'a> {
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
        let relative_path = pathdata.get_relative_path(proximate_dataset_mount)?;

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
            relative_path,
            snap_mounts,
        })
    }

    pub fn get_all_versions(&'a self) -> impl ParallelIterator<Item = PathData> + 'a {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        //
        // BTreeMap will then remove duplicates with the same system modify time and size/file len
        self
            .snap_mounts
            .par_iter()
            .map(|path| path.join(self.relative_path))
            .filter_map(|joined_path| {
                match joined_path.symlink_metadata() {
                    Ok(md) => Some(PathData::new(joined_path.as_path(), Some(md))),
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

    #[allow(clippy::mutable_key_type)]
    pub fn get_unique_versions(&self, uniqueness: &ListSnapsOfType) -> Vec<PathData> {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        //
        // BTreeMap will then remove duplicates with the same system modify time and size/file len

        let iter = self.get_all_versions();

        let sorted_versions: Vec<PathData> = match uniqueness {
            ListSnapsOfType::All => {
                let mut versions: Vec<PathData> = iter.collect();

                versions.sort_unstable();

                versions
            }
            ListSnapsOfType::UniqueContents => {
                let versions: BTreeMap<CompareFiles, PathData> = iter
                    .filter_map(|pathdata| {
                        pathdata.metadata.map(|metadata| {
                            (
                                CompareFiles {
                                    metadata,
                                    extra: Some(CompareFilesExtra {
                                        hash: OnceCell::new(),
                                        path: pathdata.path_buf.clone(),
                                    }),
                                },
                                pathdata,
                            )
                        })
                    })
                    .collect();

                versions.into_values().collect()
            }
            ListSnapsOfType::UniqueMetadata => {
                let versions: BTreeMap<CompareFiles, PathData> = iter
                    .filter_map(|pathdata| {
                        pathdata.metadata.map(|metadata| {
                            (
                                CompareFiles {
                                    metadata,
                                    extra: None,
                                },
                                pathdata,
                            )
                        })
                    })
                    .collect();

                versions.into_values().collect()
            }
        };

        sorted_versions
    }

    pub fn get_last_version(&self) -> Option<PathData> {
        let mut sorted_versions = self.get_unique_versions(&ListSnapsOfType::All);

        let res: Option<PathData> = sorted_versions.pop();

        res
    }
}

#[derive(Eq, PartialEq)]
struct CompareFiles {
    metadata: PathMetadata,
    extra: Option<CompareFilesExtra>,
}

#[derive(Eq, PartialEq)]
struct CompareFilesExtra {
    path: PathBuf,
    hash: OnceCell<u32>,
}

impl PartialOrd for CompareFiles {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CompareFiles {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if self.metadata.modify_time == other.metadata.modify_time {
            return self.metadata.size.cmp(&other.metadata.size);
        }

        // if files, differ re mtime, but have same size, we test by bytes whether the same
        if self.extra.is_some()
            && other.extra.is_some()
            && self.metadata.size == other.metadata.size
        {
            if let Ok(is_same_file) = self.is_same_file(other) {
                if is_same_file {
                    return Ordering::Equal;
                }
            }
        }

        self.metadata.modify_time.cmp(&other.metadata.modify_time)
    }
}

impl CompareFiles {
    #[inline]
    #[allow(unused_assignments)]
    fn is_same_file(&self, other: &Self) -> HttmResult<bool> {
        const IN_BUFFER_SIZE: usize = 65_536;

        let mut self_adler = Adler32::new();
        let mut other_adler = Adler32::new();

        let self_file = File::open(&self.extra.as_ref().unwrap().path)?;
        let other_file = File::open(&other.extra.as_ref().unwrap().path)?;

        let mut self_buffer = BufReader::with_capacity(IN_BUFFER_SIZE, self_file);
        let mut other_buffer = BufReader::with_capacity(IN_BUFFER_SIZE, other_file);

        let mut self_bytes_buffer = Vec::with_capacity(IN_BUFFER_SIZE);
        let mut other_bytes_buffer = Vec::with_capacity(IN_BUFFER_SIZE);

        loop {
            if self.extra.as_ref().unwrap().hash.get().is_some()
                && other.extra.as_ref().unwrap().hash.get().is_some()
            {
                break;
            }

            if self.extra.as_ref().unwrap().hash.get().is_none() {
                if let Ok(chunk) = self_buffer.fill_buf() {
                    self_bytes_buffer = chunk.to_vec();
                    self_buffer.consume(self_bytes_buffer.len());
                    self_adler.write(&self_bytes_buffer);
                }
            }

            if other.extra.as_ref().unwrap().hash.get().is_none() {
                if let Ok(chunk) = other_buffer.fill_buf() {
                    other_bytes_buffer = chunk.to_vec();
                    other_buffer.consume(other_bytes_buffer.len());
                    other_adler.write(&other_bytes_buffer);
                }
            }

            if self_bytes_buffer.is_empty() && other_bytes_buffer.is_empty() {
                break;
            }
        }

        let self_hash = self
            .extra
            .as_ref()
            .unwrap()
            .hash
            .get_or_init(|| self_adler.finish());
        let other_hash = other
            .extra
            .as_ref()
            .unwrap()
            .hash
            .get_or_init(|| other_adler.finish());

        if self_hash == other_hash {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
