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

use rayon::prelude::*;

use crate::{
    utility::{HttmError, PathData},
    AltInfo, MapOfAliases, MapOfDatasets, PathSet, SnapsAndLiveSet, VecOfSnapInfo,
};
use crate::{Config, HttmResult, SnapshotDatasetType};

#[derive(Debug, Clone)]
pub struct FileSearchBundle {
    pub relative_path: PathBuf,
    pub opt_snap_mounts: Option<VecOfSnapInfo>,
}

pub fn versions_lookup_exec(
    config: &Config,
    vec_pathdata: &PathSet,
) -> HttmResult<SnapsAndLiveSet> {
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
        && live_versions
            .par_iter()
            .all(|pathdata| pathdata.metadata.is_none())
        && !config.opt_no_snap
    {
        return Err(HttmError::new(
            "httm could not find either a live copy or a snapshot copy of any specified file, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    Ok([all_snap_versions, live_versions])
}

fn get_all_snap_versions(config: &Config, vec_pathdata: &[PathData]) -> HttmResult<Vec<PathData>> {
    // create vec of all local and replicated backups at once
    let all_snap_versions: Vec<PathData> = vec_pathdata
        .par_iter()
        .map(|pathdata| {
            config
                .dataset_collection
                .datasets_of_interest
                .par_iter()
                .flat_map(|dataset_type| get_datasets_for_search(config, pathdata, dataset_type))
                .flat_map(|dataset_for_search| {
                    get_file_search_bundle(config, pathdata, &dataset_for_search)
                })
        })
        .flatten()
        .flatten()
        .flat_map(|search_bundle| get_versions_from_snap_mounts(&search_bundle))
        .flatten()
        .collect();

    Ok(all_snap_versions)
}

pub fn get_datasets_for_search(
    config: &Config,
    pathdata: &PathData,
    requested_dataset_type: &SnapshotDatasetType,
) -> HttmResult<AltInfo> {
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
        Some(map_of_aliases) => match get_alias_dataset(pathdata, map_of_aliases) {
            Some(alias_snap_dir) => alias_snap_dir,
            None => get_proximate_dataset(pathdata, &config.dataset_collection.map_of_datasets)?,
        },
        None => get_proximate_dataset(pathdata, &config.dataset_collection.map_of_datasets)?,
    };

    let datasets_for_search: AltInfo = match requested_dataset_type {
        SnapshotDatasetType::MostProximate => {
            // just return the same dataset when in most proximate mode
            AltInfo {
                proximate_dataset_mount: proximate_dataset_mount.clone(),
                datasets_of_interest: vec![proximate_dataset_mount],
            }
        }
        SnapshotDatasetType::AltReplicated => match &config.dataset_collection.opt_map_of_alts {
            Some(map_of_alts) => match map_of_alts.get(proximate_dataset_mount.as_path()) {
                Some(datasets_for_search) => datasets_for_search.to_owned(),
                None => return Err(HttmError::new("If you are here a map of alts is missing for a supplied mount, \
                this is fine as we should just flatten/ignore this error.").into()),
            },
            None => unreachable!("If config option alt-replicated is specified, then a map of alts should have been generated, \
            if you are here such a map is missing."),
        },
    };

    Ok(datasets_for_search)
}

pub fn get_file_search_bundle(
    config: &Config,
    pathdata: &PathData,
    datasets_for_search: &AltInfo,
) -> HttmResult<Vec<FileSearchBundle>> {
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

            let relative_path = get_relative_path(config, pathdata, proximate_dataset_mount)?;

            let opt_snap_mounts = config
                .dataset_collection
                .map_of_snaps
                .get(dataset_of_interest)
                .cloned();

            Ok(FileSearchBundle {
                relative_path,
                opt_snap_mounts,
            })
        })
        .collect()
}

fn get_relative_path(
    config: &Config,
    pathdata: &PathData,
    proximate_dataset_mount: &Path,
) -> HttmResult<PathBuf> {
    let default_path_strip = || pathdata.path_buf.strip_prefix(&proximate_dataset_mount);

    let relative_path = match &config.dataset_collection.opt_map_of_aliases {
        Some(map_of_aliases) => {
            let opt_aliased_local_dir = map_of_aliases
                .par_iter()
                // do a search for a key with a value
                .find_map_first(|(local_dir, alias_info)| {
                    if alias_info.remote_dir == proximate_dataset_mount {
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

    Ok(relative_path.to_path_buf())
}

fn get_alias_dataset(pathdata: &PathData, map_of_alias: &MapOfAliases) -> Option<PathBuf> {
    let ancestors: Vec<&Path> = pathdata.path_buf.ancestors().collect();

    // find_map_first should return the first seq result with a par_iter
    // but not with a par_bridge
    ancestors.into_par_iter().find_map_first(|ancestor| {
        map_of_alias
            .get(ancestor)
            .map(|alias_info| alias_info.remote_dir.clone())
    })
}

fn get_proximate_dataset(
    pathdata: &PathData,
    map_of_datasets: &MapOfDatasets,
) -> HttmResult<PathBuf> {
    // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
    // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
    // we map to return the key, instead of the value
    let ancestors: Vec<&Path> = pathdata.path_buf.ancestors().collect();

    let opt_best_potential_mountpoint: Option<PathBuf> =
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        ancestors.into_par_iter().find_map_first(|ancestor| {
            if map_of_datasets
                .contains_key(ancestor){
                    Some(ancestor)
                } else {
                    None
                }
        }).map(|path| path.to_path_buf());

    // do we have any mount points left? if not print error
    opt_best_potential_mountpoint.ok_or_else(|| {
        HttmError::new(
            "httm could not identify any qualifying dataset.  \
            Maybe consider specifying manually at SNAP_POINT?",
        )
        .into()
    })
}

fn get_versions_from_snap_mounts(search_bundle: &FileSearchBundle) -> HttmResult<Vec<PathData>> {
    // get the DirEntry for our snapshot path which will have all our possible
    // snapshots, like so: .zfs/snapshots/<some snap name>/
    //
    // BTreeMap will then remove duplicates with the same system modify time and size/file len

    let snap_mounts: &VecOfSnapInfo = search_bundle.opt_snap_mounts.as_ref().ok_or_else(|| {
        HttmError::new(
            "If you are here, httm could find no snap mount for your files.  \
        Iterator should just ignore/flatten the error.",
        )
    })?;

    let sorted_versions: Vec<PathData> =
        get_unique_versions(snap_mounts, &search_bundle.relative_path)?
            .into_values()
            .collect();

    Ok(sorted_versions)
}

fn get_unique_versions(
    snap_mounts: &[PathBuf],
    relative_path: &Path,
) -> HttmResult<BTreeMap<(SystemTime, u64), PathData>> {
    let unique_versions = snap_mounts
        .par_iter()
        .map(|path| path.join(&relative_path))
        .map(|joined_path| PathData::from(joined_path.as_path()))
        .filter_map(|pathdata| {
            pathdata
                .metadata
                .map(|metadata| ((metadata.modify_time, metadata.size), pathdata))
        })
        .collect();

    Ok(unique_versions)
}
