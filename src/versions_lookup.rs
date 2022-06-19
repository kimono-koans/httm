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
    fs::read_dir,
    path::{Path, PathBuf},
    time::SystemTime,
};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;

use crate::{
    Config, FilesystemType, HttmError, PathData, SnapPoint, BTRFS_SNAPPER_HIDDEN_DIRECTORY,
    BTRFS_SNAPPER_SUFFIX, ZFS_SNAPSHOT_DIRECTORY,
};

pub struct DatasetsForSearch {
    pub proximate_dataset_mount: PathBuf,
    pub datasets_of_interest: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum NativeDatasetType {
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

pub fn get_versions_set(
    config: &Config,
    vec_pathdata: &Vec<PathData>,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups on alt replicated sets if necessary
    let selected_datasets = if config.opt_alt_replicated {
        vec![
            NativeDatasetType::AltReplicated,
            NativeDatasetType::MostProximate,
        ]
    } else {
        vec![NativeDatasetType::MostProximate]
    };

    let all_snap_versions: Vec<PathData> =
        get_all_snap_versions(config, vec_pathdata, &selected_datasets)?;

    // create vec of live copies - unless user doesn't want it!
    let live_versions: Vec<PathData> = if !config.opt_no_live_vers {
        vec_pathdata.clone()
    } else {
        Vec::new()
    };

    // check if all files (snap and live) do not exist, if this is true, then user probably messed up
    // and entered a file that never existed (that is, perhaps a wrong file name)?
    if all_snap_versions.is_empty() && live_versions.par_iter().all(|i| i.is_phantom) {
        return Err(HttmError::new(
            "Neither a live copy, nor a snapshot copy of such a file appears to exist, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    Ok([all_snap_versions, live_versions])
}

fn get_all_snap_versions(
    config: &Config,
    vec_pathdata: &Vec<PathData>,
    selected_datasets: &Vec<NativeDatasetType>,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // create vec of all local and replicated backups at once
    let all_snap_versions: Vec<PathData> = vec_pathdata
        .par_iter()
        .map(|path_data| {
            selected_datasets
                .par_iter()
                .map(|dataset_type| get_search_bundle(config, path_data, dataset_type))
                .flatten()
        })
        .flatten()
        .flatten()
        .flat_map(|search_bundle| get_versions_per_dataset(config, &search_bundle))
        .flatten()
        .collect();

    Ok(all_snap_versions)
}

pub fn get_search_bundle(
    config: &Config,
    pathdata: &PathData,
    requested_dataset_type: &NativeDatasetType,
) -> Result<Vec<SearchBundle>, Box<dyn std::error::Error + Send + Sync + 'static>> {
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
    let dataset_collection: DatasetsForSearch = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => DatasetsForSearch {
            proximate_dataset_mount: defined_dirs.snap_dir.clone(),
            datasets_of_interest: vec![defined_dirs.snap_dir.clone()],
        },
        SnapPoint::Native(native_datasets) => {
            let proximate_dataset_mount =
                get_proximate_dataset(pathdata, &native_datasets.map_of_datasets)?;
            match requested_dataset_type {
                NativeDatasetType::MostProximate => {
                    // just return the same dataset when in most proximate mode
                    DatasetsForSearch {
                        proximate_dataset_mount: proximate_dataset_mount.clone(),
                        datasets_of_interest: vec![proximate_dataset_mount],
                    }
                }
                NativeDatasetType::AltReplicated => match &native_datasets.map_of_alts {
                    Some(map_of_alts) => match map_of_alts.get(&proximate_dataset_mount) {
                        Some(alternate_mounts) => DatasetsForSearch {
                            proximate_dataset_mount,
                            datasets_of_interest: alternate_mounts.clone(),
                        },
                        None => get_alt_replicated_datasets(
                            &proximate_dataset_mount,
                            &native_datasets.map_of_datasets,
                        )?,
                    },
                    None => get_alt_replicated_datasets(
                        &proximate_dataset_mount,
                        &native_datasets.map_of_datasets,
                    )?,
                },
            }
        }
    };

    dataset_collection
        .datasets_of_interest
        .par_iter()
        .map(|dataset_of_interest| {
            // building our relative path by removing parent below the snap dir
            //
            // for native searches the prefix is are the dirs below the most proximate dataset
            // for user specified dirs these are specified by the user

            let proximate_dataset_mount = &dataset_collection.proximate_dataset_mount;

            let (snapshot_dir, relative_path, snapshot_mounts, fs_type) = match &config.snap_point {
                SnapPoint::UserDefined(defined_dirs) => {
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
                SnapPoint::Native(native_datasets) => {
                    // this prefix removal is why we always need the proximate dataset name, even when we are searching an alternate replicated filesystem

                    // building the snapshot path from our dataset
                    let (snapshot_dir, fs_type) =
                        match &native_datasets.map_of_datasets.get(dataset_of_interest) {
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

                    let snapshot_mounts = match &native_datasets.map_of_snaps {
                        Some(map_of_snaps) => map_of_snaps.get(dataset_of_interest).cloned(),
                        None => None,
                    };

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
    // we map to return the key mount
    let opt_best_potential_mountpoint: Option<PathBuf> = pathdata
        .path_buf
        .ancestors()
        .par_bridge()
        .find_map_first(|ancestor| {
            map_of_datasets
                .get(ancestor)
                .map(|_| ancestor.to_path_buf())
        });

    // do we have any mount points left? if not print error
    match opt_best_potential_mountpoint {
        Some(best_potential_mountpoint) => Ok(best_potential_mountpoint),
        None => {
            let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?";
            Err(HttmError::new(msg).into())
        }
    }
}

pub fn get_alt_replicated_datasets(
    proximate_dataset_mount: &Path,
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> Result<DatasetsForSearch, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let proximate_dataset_fsname = match &map_of_datasets.get(proximate_dataset_mount) {
        Some((proximate_dataset_fsname, _)) => proximate_dataset_fsname.to_string(),
        None => {
            return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
        }
    };

    // find a filesystem that ends with our most local filesystem name
    // but which has a prefix, like a different pool name: rpool might be
    // replicated to tank/rpool
    let mut alt_replicated_mounts: Vec<PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, (fs_name, _fstype))| {
            fs_name != &proximate_dataset_fsname
                && fs_name.ends_with(proximate_dataset_fsname.as_str())
        })
        .map(|(mount, _fsname)| mount)
        .cloned()
        .collect();

    if alt_replicated_mounts.is_empty() {
        // could not find the any replicated mounts
        Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
    } else {
        alt_replicated_mounts.sort_unstable_by_key(|path| path.as_os_str().len());
        Ok(DatasetsForSearch {
            proximate_dataset_mount: proximate_dataset_mount.to_path_buf(),
            datasets_of_interest: alt_replicated_mounts,
        })
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

    let snapshot_dir = search_bundle.snapshot_dir.as_ref();
    let relative_path = search_bundle.relative_path.as_ref();
    let fs_type = &search_bundle.fs_type;

    // this is the fallback/non-Linux way of handling without a map_of_snaps
    fn read_dir_for_datasets(
        snapshot_dir: &Path,
        relative_path: &Path,
        fs_type: &FilesystemType,
    ) -> Result<
        HashMap<(SystemTime, u64), PathData>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_versions = read_dir(match fs_type {
            FilesystemType::Btrfs => snapshot_dir.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY),
            FilesystemType::Zfs => snapshot_dir.to_path_buf(),
        })?
        .flatten()
        .par_bridge()
        .map(|entry| match fs_type {
            FilesystemType::Btrfs => entry.path().join(BTRFS_SNAPPER_SUFFIX),
            FilesystemType::Zfs => entry.path(),
        })
        .map(|path| path.join(relative_path))
        .map(|joined_path| PathData::from(joined_path.as_path()))
        .filter(|pathdata| !pathdata.is_phantom)
        .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
        .collect();

        Ok(unique_versions)
    }

    fn snap_mounts_for_datasets(
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

    let unique_versions: HashMap<(SystemTime, u64), PathData> = match &config.snap_point {
        SnapPoint::Native(native_datasets) => match native_datasets.map_of_snaps {
            // Do we have a map_of snaps? If so, get_search_bundle function has already prepared the ones
            // we actually need for this dataset so we can skip the unwrap.
            Some(_) => match search_bundle.snapshot_mounts.as_ref() {
                Some(snap_mounts) => snap_mounts_for_datasets(snap_mounts, relative_path)?,
                None => read_dir_for_datasets(snapshot_dir, relative_path, fs_type)?,
            },
            None => read_dir_for_datasets(snapshot_dir, relative_path, fs_type)?,
        },
        SnapPoint::UserDefined(user_defined_dirs) => {
            read_dir_for_datasets(snapshot_dir, relative_path, &user_defined_dirs.fs_type)?
        }
    };

    let mut vec_pathdata: Vec<PathData> = unique_versions.into_par_iter().map(|(_, v)| v).collect();

    vec_pathdata.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(vec_pathdata)
}
