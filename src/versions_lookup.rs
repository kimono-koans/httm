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
    fs::read_dir,
    path::{Path, PathBuf},
    time::SystemTime,
};

use itertools::Itertools;
use rayon::prelude::*;

use crate::{
    display::{display_exec, display_ordered_map},
    utility::{print_output_buf, HttmError, PathData},
};
use crate::{
    AHashMap as HashMap, Config, DatasetCollection, FilesystemType, BTRFS_SNAPPER_HIDDEN_DIRECTORY,
    BTRFS_SNAPPER_SUFFIX, ZFS_SNAPSHOT_DIRECTORY,
};

#[derive(Debug, Clone)]
pub struct DatasetsForSearch {
    pub proximate_dataset_mount: PathBuf,
    pub datasets_of_interest: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum SnapshotDatasetType {
    MostProximate,
    AltReplicated,
}

#[derive(Debug, Clone)]
pub struct SearchBundle {
    pub snapshot_dir: PathBuf,
    pub relative_path: PathBuf,
    pub fs_type: FilesystemType,
    pub snapshot_mounts: Option<Vec<PathBuf>>,
}

pub fn versions_lookup_exec(
    config: &Config,
    vec_pathdata: &[PathData],
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups on alt replicated sets if necessary
    let selected_datasets = if config.opt_alt_replicated {
        vec![
            SnapshotDatasetType::AltReplicated,
            SnapshotDatasetType::MostProximate,
        ]
    } else {
        vec![SnapshotDatasetType::MostProximate]
    };

    let all_snap_versions: Vec<PathData> = if config.opt_mount_for_file {
        display_mounts_for_files(config, vec_pathdata, &selected_datasets)?;
        unreachable!("Should never return back from display_mounts_for_files")
    } else if config.opt_no_snap {
        Vec::new()
    } else {
        get_all_snap_versions(config, vec_pathdata, &selected_datasets)?
    };

    // create vec of live copies - unless user doesn't want it!
    let live_versions: Vec<PathData> = if config.opt_no_live {
        Vec::new()
    } else {
        vec_pathdata.to_owned()
    };

    // check if all files (snap and live) do not exist, if this is true, then user probably messed up
    // and entered a file that never existed (that is, perhaps a wrong file name)?
    if all_snap_versions.is_empty()
        && live_versions.par_iter().all(|pathdata| pathdata.is_phantom)
        && !config.opt_no_snap
    {
        return Err(HttmError::new(
            "httm could not find either a live copy or a snapshot copy of any specified file, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    Ok([all_snap_versions, live_versions])
}

fn display_mounts_for_files(
    config: &Config,
    vec_pathdata: &[PathData],
    selected_datasets: &[SnapshotDatasetType],
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mounts_for_files = get_mounts_for_files(config, vec_pathdata, selected_datasets)?;

    let output_buf = if config.opt_raw || config.opt_zeros {
        display_exec(
            config,
            &[
                mounts_for_files.into_values().flatten().collect(),
                Vec::new(),
            ],
        )?
    } else {
        display_ordered_map(config, &mounts_for_files)?
    };

    print_output_buf(output_buf)?;

    std::process::exit(0)
}

#[allow(clippy::type_complexity)]
pub fn get_mounts_for_files(
    config: &Config,
    vec_pathdata: &[PathData],
    selected_datasets: &[SnapshotDatasetType],
) -> Result<BTreeMap<PathData, Vec<PathData>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // we only check for phantom files in "mount for file" mode because
    // people should be able to search for deleted files in other modes
    let (phantom_files, non_phantom_files): (Vec<&PathData>, Vec<&PathData>) = vec_pathdata
        .par_iter()
        .partition(|pathdata| pathdata.is_phantom);

    if !phantom_files.is_empty() {
        eprintln!(
            "httm was unable to determine mount locations for all input files, \
        because the following files do not appear to exist: "
        );

        phantom_files
            .iter()
            .for_each(|pathdata| eprintln!("{}", pathdata.path_buf.to_string_lossy()));
    }

    let mounts_for_files: BTreeMap<PathData, Vec<PathData>> = non_phantom_files
        .into_iter()
        .map(|pathdata| {
            let datasets: Vec<DatasetsForSearch> = selected_datasets
                .iter()
                .flat_map(|dataset_type| get_datasets_for_search(config, pathdata, dataset_type))
                .collect();
            (pathdata, datasets)
        })
        .into_group_map_by(|(pathdata, _datasets_for_search)| pathdata.to_owned())
        .into_iter()
        .map(|(pathdata, vec_datasets_for_search)| {
            let datasets: Vec<PathData> = vec_datasets_for_search
                .into_iter()
                .flat_map(|(_proximate_mount, datasets_for_search)| datasets_for_search)
                .flat_map(|datasets_for_search| datasets_for_search.datasets_of_interest)
                .map(|path| PathData::from(path.as_path()))
                .rev()
                .collect();
            (pathdata.to_owned(), datasets)
        })
        .collect();

    Ok(mounts_for_files)
}

fn get_all_snap_versions(
    config: &Config,
    vec_pathdata: &[PathData],
    selected_datasets: &[SnapshotDatasetType],
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // create vec of all local and replicated backups at once
    let all_snap_versions: Vec<PathData> = vec_pathdata
        .par_iter()
        .map(|pathdata| {
            selected_datasets.par_iter().flat_map(|dataset_type| {
                let dataset_for_search = get_datasets_for_search(config, pathdata, dataset_type)?;
                get_search_bundle(config, pathdata, &dataset_for_search)
            })
        })
        .flatten()
        .flatten()
        .flat_map(|search_bundle| get_versions_per_dataset(config, &search_bundle))
        .flatten()
        .collect();

    Ok(all_snap_versions)
}

pub fn get_datasets_for_search(
    config: &Config,
    pathdata: &PathData,
    requested_dataset_type: &SnapshotDatasetType,
) -> Result<DatasetsForSearch, Box<dyn std::error::Error + Send + Sync + 'static>> {
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
    let datasets_for_search: DatasetsForSearch = match &config.dataset_collection {
        DatasetCollection::UserDefined(defined_dirs) => {
            let snap_dir = defined_dirs.snap_dir.to_path_buf();
            DatasetsForSearch {
                proximate_dataset_mount: snap_dir.clone(),
                datasets_of_interest: vec![snap_dir],
            }
        }
        DatasetCollection::AutoDetect(detected_datasets) => {
            let proximate_dataset_mount =
                get_proximate_dataset(pathdata, &detected_datasets.map_of_datasets)?;
            match requested_dataset_type {
                SnapshotDatasetType::MostProximate => {
                    // just return the same dataset when in most proximate mode
                    DatasetsForSearch {
                        proximate_dataset_mount: proximate_dataset_mount.clone(),
                        datasets_of_interest: vec![proximate_dataset_mount],
                    }
                }
                SnapshotDatasetType::AltReplicated => match &detected_datasets.opt_map_of_alts {
                    Some(map_of_alts) => match map_of_alts.get(proximate_dataset_mount.as_path()) {
                        Some(alternate_mounts) => DatasetsForSearch {
                            proximate_dataset_mount,
                            datasets_of_interest: alternate_mounts.clone(),
                        },
                        None => return Err(HttmError::new("If you are here a map of alts is missing for a supplied mount, \
                        this is fine as we should just flatten/ignore this error.").into()),
                    },
                    None => unreachable!("If config option alt-replicated is specified, then a map of alts should have been generated, \
                    if you are here such a map is missing."),
                },
            }
        }
    };

    Ok(datasets_for_search)
}

pub fn get_search_bundle(
    config: &Config,
    pathdata: &PathData,
    datasets_for_search: &DatasetsForSearch,
) -> Result<Vec<SearchBundle>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    datasets_for_search
        .datasets_of_interest
        .par_iter()
        .map(|dataset_of_interest| {
            // building our relative path by removing parent below the snap dir
            //
            // for native searches the prefix is are the dirs below the most proximate dataset
            // for user specified dirs these are specified by the user
            let proximate_dataset_mount = &datasets_for_search.proximate_dataset_mount;

            let (snapshot_dir, relative_path, snapshot_mounts, fs_type) =
                match &config.dataset_collection {
                    DatasetCollection::UserDefined(defined_dirs) => {
                        let (snapshot_dir, fs_type) = match &defined_dirs.fs_type {
                            FilesystemType::Zfs => (
                                dataset_of_interest.join(ZFS_SNAPSHOT_DIRECTORY),
                                FilesystemType::Zfs,
                            ),
                            FilesystemType::Btrfs => {
                                (dataset_of_interest.to_path_buf(), FilesystemType::Btrfs)
                            }
                        };

                        let relative_path = pathdata
                            .path_buf
                            .strip_prefix(&defined_dirs.local_dir)?
                            .to_path_buf();

                        let snapshot_mounts = None;

                        (snapshot_dir, relative_path, snapshot_mounts, fs_type)
                    }
                    DatasetCollection::AutoDetect(detected_datasets) => {
                        // this prefix removal is why we always need the proximate dataset name, even when we are searching an alternate replicated filesystem

                        // building the snapshot path from our dataset
                        let (snapshot_dir, fs_type) =
                            match &detected_datasets.map_of_datasets.get(dataset_of_interest) {
                                Some((_, fstype)) => match fstype {
                                    FilesystemType::Zfs => (
                                        dataset_of_interest.join(ZFS_SNAPSHOT_DIRECTORY),
                                        FilesystemType::Zfs,
                                    ),
                                    FilesystemType::Btrfs => {
                                        (dataset_of_interest.to_path_buf(), FilesystemType::Btrfs)
                                    }
                                },
                                None => (
                                    dataset_of_interest.join(ZFS_SNAPSHOT_DIRECTORY),
                                    FilesystemType::Zfs,
                                ),
                            };

                        let relative_path = pathdata
                            .path_buf
                            .strip_prefix(&proximate_dataset_mount)?
                            .to_path_buf();

                        let snapshot_mounts = detected_datasets
                            .map_of_snaps
                            .get(dataset_of_interest)
                            .cloned();

                        (snapshot_dir, relative_path, snapshot_mounts, fs_type)
                    }
                };

            Ok(SearchBundle {
                snapshot_dir,
                relative_path,
                snapshot_mounts,
                fs_type,
            })
        })
        .collect()
}

fn get_proximate_dataset(
    pathdata: &PathData,
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
    // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
    // we map to return the key, instead of the value
    let ancestors: Vec<&Path> = pathdata.path_buf.ancestors().collect();

    let opt_best_potential_mountpoint: Option<&Path> =
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        ancestors.into_par_iter().find_map_first(|ancestor| {
            if map_of_datasets
                .contains_key(ancestor){
                    Some(ancestor)
                } else {
                    None
                }
        });

    // do we have any mount points left? if not print error
    match opt_best_potential_mountpoint.map(|path| path.to_path_buf()) {
        Some(best_potential_mountpoint) => Ok(best_potential_mountpoint),
        None => {
            let msg = "httm could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?";
            Err(HttmError::new(msg).into())
        }
    }
}

fn get_versions_per_dataset(
    config: &Config,
    search_bundle: &SearchBundle,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get the DirEntry for our snapshot path which will have all our possible
    // snapshots, like so: .zfs/snapshots/<some snap name>/
    //
    // hashmap will then remove duplicates with the same system modify time and size/file len

    // this is the fallback way of handling without a map_of_snaps, if all we have is user defined dirs
    fn versions_from_read_dir(
        snapshot_dir: &Path,
        relative_path: &Path,
        fs_type: &FilesystemType,
    ) -> Result<
        HashMap<(SystemTime, u64), PathData>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_versions = prep_lookup_read_dir(snapshot_dir, relative_path, fs_type)?
            .par_iter()
            .map(|joined_path| PathData::from(joined_path.as_path()))
            .filter(|pathdata| !pathdata.is_phantom)
            .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
            .collect();

        Ok(unique_versions)
    }

    // this is the optimal way to handle for native datasets, if you have a map_of_snaps
    fn versions_from_snap_mounts(
        snap_mounts: &[PathBuf],
        relative_path: &Path,
    ) -> Result<
        HashMap<(SystemTime, u64), PathData>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_versions = snap_mounts
            .par_iter()
            .map(|path| path.join(&relative_path))
            .map(|path| PathData::from(path.as_path()))
            .filter(|pathdata| !pathdata.is_phantom)
            .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
            .collect();
        Ok(unique_versions)
    }

    let (snapshot_dir, relative_path, snapshot_mounts, fs_type) = {
        (
            &search_bundle.snapshot_dir,
            &search_bundle.relative_path,
            &search_bundle.snapshot_mounts,
            &search_bundle.fs_type,
        )
    };

    let unique_versions: HashMap<(SystemTime, u64), PathData> = match &config.dataset_collection {
        DatasetCollection::AutoDetect(_) => {
            match snapshot_mounts {
                Some(snap_mounts) => versions_from_snap_mounts(snap_mounts, relative_path)?,
                // snap mounts is empty
                None => {
                    return Err(HttmError::new(
                        "If you are here, precompute showed no snap mounts for dataset.  \
                    Iterator should just ignore/flatten the error.",
                    )
                    .into());
                }
            }
        }
        DatasetCollection::UserDefined(_) => {
            versions_from_read_dir(snapshot_dir, relative_path, fs_type)?
        }
    };

    let mut vec_pathdata: Vec<PathData> = unique_versions.into_values().collect();

    vec_pathdata.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(vec_pathdata)
}

pub fn prep_lookup_read_dir(
    snapshot_dir: &Path,
    relative_path: &Path,
    fs_type: &FilesystemType,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let paths = read_dir(match fs_type {
        FilesystemType::Btrfs => snapshot_dir.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY),
        FilesystemType::Zfs => snapshot_dir.to_path_buf(),
    })?
    .flatten()
    .map(|entry| match fs_type {
        FilesystemType::Btrfs => entry.path().join(BTRFS_SNAPPER_SUFFIX),
        FilesystemType::Zfs => entry.path(),
    })
    .map(|path| path.join(relative_path))
    .collect();

    Ok(paths)
}
