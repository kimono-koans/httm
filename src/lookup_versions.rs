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
    path::{Path, PathBuf},
    time::SystemTime,
};

use itertools::Itertools;
use rayon::prelude::*;

use crate::{
    utility::{HttmError, PathData},
    ExecMode,
};
use crate::{AHashMap as HashMap, Config, FilesystemType, SnapshotDatasetType};

#[derive(Debug, Clone, Hash, PartialEq)]
pub struct DatasetsForSearch {
    pub proximate_dataset_mount: PathBuf,
    pub datasets_of_interest: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FileSearchBundle {
    pub relative_path: PathBuf,
    pub opt_snap_mounts: Option<Vec<PathBuf>>,
}

pub fn versions_lookup_exec(
    config: &Config,
    vec_pathdata: &[PathData],
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    let all_snap_versions: Vec<PathData> = if config.opt_no_snap {
        Vec::new()
    } else {
        get_all_snap_versions(config, vec_pathdata)?
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

#[allow(clippy::type_complexity)]
pub fn get_mounts_for_files(
    config: &Config,
) -> Result<BTreeMap<PathData, Vec<PathData>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // we only check for phantom files in "mount for file" mode because
    // people should be able to search for deleted files in other modes
    let (phantom_files, non_phantom_files): (Vec<&PathData>, Vec<&PathData>) = config
        .paths
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
            // don't want to request alt replicated mounts in snap mode, though we may in opt_mount_for_file mode
            let selected_datasets = if config.exec_mode == ExecMode::SnapFileMount {
                vec![SnapshotDatasetType::MostProximate]
            } else {
                config.datasets_of_interest.clone()
            };

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
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // create vec of all local and replicated backups at once
    let all_snap_versions: Vec<PathData> = vec_pathdata
        .par_iter()
        .map(|pathdata| {
            config
                .datasets_of_interest
                .par_iter()
                .flat_map(|dataset_type| {
                    let dataset_for_search =
                        get_datasets_for_search(config, pathdata, dataset_type)?;
                    get_search_bundle(config, pathdata, &dataset_for_search)
                })
        })
        .flatten()
        .flatten()
        .flat_map(|search_bundle| get_versions_per_dataset(&search_bundle))
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
    let datasets_for_search: DatasetsForSearch = {
        let proximate_dataset_mount = match &config.dataset_collection.opt_map_of_aliases {
            Some(map_of_aliases) => match get_alias_dataset(pathdata, map_of_aliases) {
                Some(snap_dir) => snap_dir,
                None => {
                    get_proximate_dataset(pathdata, &config.dataset_collection.map_of_datasets)?
                }
            },
            None => get_proximate_dataset(pathdata, &config.dataset_collection.map_of_datasets)?,
        };

        match requested_dataset_type {
            SnapshotDatasetType::MostProximate => {
                // just return the same dataset when in most proximate mode
                DatasetsForSearch {
                    proximate_dataset_mount: proximate_dataset_mount.clone(),
                    datasets_of_interest: vec![proximate_dataset_mount],
                }
            }
            SnapshotDatasetType::AltReplicated => match &config.dataset_collection.opt_map_of_alts {
                Some(map_of_alts) => match map_of_alts.get(proximate_dataset_mount.as_path()) {
                    Some(datasets_for_search) => datasets_for_search.clone(),
                    None => return Err(HttmError::new("If you are here a map of alts is missing for a supplied mount, \
                    this is fine as we should just flatten/ignore this error.").into()),
                },
                None => unreachable!("If config option alt-replicated is specified, then a map of alts should have been generated, \
                if you are here such a map is missing."),
            },
        }
    };

    Ok(datasets_for_search)
}

pub fn get_search_bundle(
    config: &Config,
    pathdata: &PathData,
    datasets_for_search: &DatasetsForSearch,
) -> Result<Vec<FileSearchBundle>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    datasets_for_search
        .datasets_of_interest
        .par_iter()
        .map(|dataset_of_interest| {
            // building our relative path by removing parent below the snap dir
            //
            // for native searches the prefix is are the dirs below the most proximate dataset
            // for user specified dirs these are specified by the user
            let proximate_dataset_mount = &datasets_for_search.proximate_dataset_mount;
            // this prefix removal is why we always need the proximate dataset name, even when we are searching an alternate replicated filesystem

            let default_path_strip = || pathdata.path_buf.strip_prefix(&proximate_dataset_mount);

            let relative_path = match &config.dataset_collection.opt_map_of_aliases {
                Some(map_of_aliases) => {
                    let opt_aliased_local_dir = map_of_aliases
                        .par_iter()
                        // do a search for a key with a value
                        .find_map_first(|(local_dir, (snap_dir, _fs_type))| {
                            if snap_dir == proximate_dataset_mount {
                                Some(local_dir)
                            } else {
                                None
                            }
                        });

                    // fallback if unable to find an alias or strip a prefix
                    // (each an indication we should not be trying aliases)
                    match opt_aliased_local_dir {
                        Some(local_dir) => match pathdata.path_buf.strip_prefix(&local_dir) {
                            Ok(alias_stripped_path) => alias_stripped_path,
                            Err(_) => default_path_strip()?,
                        },
                        None => default_path_strip()?,
                    }
                }
                None => default_path_strip()?,
            };

            let opt_snap_mounts = config
                .dataset_collection
                .map_of_snaps
                .get(dataset_of_interest)
                .cloned();

            Ok(FileSearchBundle {
                relative_path: relative_path.to_path_buf(),
                opt_snap_mounts,
            })
        })
        .collect()
}

fn get_alias_dataset(
    pathdata: &PathData,
    map_of_datasets: &HashMap<PathBuf, (PathBuf, FilesystemType)>,
) -> Option<PathBuf> {
    let ancestors: Vec<&Path> = pathdata.path_buf.ancestors().collect();

    let opt_best_potential_alias: Option<&PathBuf> =
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        ancestors.into_par_iter().find_map_first(|ancestor| {
            map_of_datasets.get(ancestor).map(|(snap_dir, _fs_type)| snap_dir)
        });

    opt_best_potential_alias.cloned()
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
    search_bundle: &FileSearchBundle,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get the DirEntry for our snapshot path which will have all our possible
    // snapshots, like so: .zfs/snapshots/<some snap name>/
    //
    // hashmap will then remove duplicates with the same system modify time and size/file len

    fn get_versions(
        snap_mounts: &[PathBuf],
        relative_path: &Path,
    ) -> Result<
        HashMap<(SystemTime, u64), PathData>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let unique_versions = snap_mounts
            .par_iter()
            .flat_map(|path| {
                let path = path.join(&relative_path);
                let opt_metadata = path.metadata().ok();

                opt_metadata.map(|metadata| (path, Some(metadata)))
            })
            .map(|(path, opt_metadata)| PathData::from_parts(path.as_path(), opt_metadata))
            .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
            .collect();
        Ok(unique_versions)
    }

    let (relative_path, opt_snap_mounts) =
        { (&search_bundle.relative_path, &search_bundle.opt_snap_mounts) };

    let snap_mounts = match opt_snap_mounts {
        Some(snap_mounts) => snap_mounts.clone(),
        // snap mounts is empty
        None => {
            return Err(HttmError::new(
                "If you are here, httm could find no snap mount for your files.  \
            Iterator should just ignore/flatten the error.",
            )
            .into());
        }
    };

    let unique_versions: HashMap<(SystemTime, u64), PathData> =
        get_versions(&snap_mounts, relative_path)?;

    let mut vec_pathdata: Vec<PathData> = unique_versions.into_values().collect();

    vec_pathdata.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(vec_pathdata)
}
