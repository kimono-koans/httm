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

use crate::utility::{HttmError, PathData};
use crate::{
    Config, HttmResult, MapOfAliases, MapOfDatasets, MostProximateAndOptAlts, SnapDatasetType,
    SnapsAndLiveSet, VecOfSnaps,
};

#[derive(Debug, Clone)]
pub struct RelativePathAndSnapMounts {
    pub relative_path: PathBuf,
    pub snap_mounts: VecOfSnaps,
}

pub fn versions_lookup_exec(config: &Config, path_set: &[PathData]) -> HttmResult<SnapsAndLiveSet> {
    let snap_versions: Vec<PathData> = if config.opt_no_snap {
        Vec::new()
    } else {
        get_all_versions_for_path_set(config, path_set)?
    };

    // create vec of live copies - unless user doesn't want it!
    let live_versions: Vec<PathData> = if config.opt_no_live {
        Vec::new()
    } else {
        path_set.to_owned()
    };

    // check if all files (snap and live) do not exist, if this is true, then user probably messed up
    // and entered a file that never existed (that is, perhaps a wrong file name)?
    if snap_versions.is_empty()
        && live_versions
            .iter()
            .all(|pathdata| pathdata.metadata.is_none())
        && !config.opt_no_snap
    {
        return Err(HttmError::new(
            "httm could not find either a live copy or a snapshot copy of any specified file, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    Ok([snap_versions, live_versions])
}

fn get_all_versions_for_path_set(
    config: &Config,
    path_set: &[PathData],
) -> HttmResult<Vec<PathData>> {
    // create vec of all local and replicated backups at once
    let snaps_selected_for_search = config
        .dataset_collection
        .snaps_selected_for_search
        .value();
    
    let all_snap_versions: Vec<PathData> = path_set
        .par_iter()
        .map(|pathdata| {
            snaps_selected_for_search
                .par_iter() 
                .flat_map(|dataset_type| select_search_datasets(config, pathdata, dataset_type))
                .flat_map(|dataset_for_search| {
                    get_version_search_bundles(config, pathdata, &dataset_for_search)
                })
        })
        .flatten()
        .flatten()
        .flat_map(|search_bundle| get_versions(&search_bundle))
        .flatten()
        .collect();

    Ok(all_snap_versions)
}

pub fn select_search_datasets(
    config: &Config,
    pathdata: &PathData,
    requested_dataset_type: &SnapDatasetType,
) -> HttmResult<MostProximateAndOptAlts> {
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

    let snap_types_for_search: MostProximateAndOptAlts = match requested_dataset_type {
        SnapDatasetType::MostProximate => {
            // just return the same dataset when in most proximate mode
            MostProximateAndOptAlts {
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

pub fn get_version_search_bundles(
    config: &Config,
    pathdata: &PathData,
    snap_types_of_interest: &MostProximateAndOptAlts,
) -> HttmResult<Vec<RelativePathAndSnapMounts>> {
    let proximate_dataset_mount = &snap_types_of_interest.proximate_dataset_mount;

    match &snap_types_of_interest.opt_datasets_of_interest {
        Some(datasets_of_interest) => datasets_of_interest
            .iter()
            .map(|dataset_of_interest| {
                get_version_search_bundle_per_dataset(
                    config,
                    pathdata,
                    proximate_dataset_mount,
                    dataset_of_interest,
                )
            })
            .collect(),
        None => [proximate_dataset_mount]
            .into_iter()
            .map(|dataset_of_interest| {
                get_version_search_bundle_per_dataset(
                    config,
                    pathdata,
                    proximate_dataset_mount,
                    dataset_of_interest,
                )
            })
            .collect(),
    }
}

fn get_version_search_bundle_per_dataset(
    config: &Config,
    pathdata: &PathData,
    proximate_dataset_mount: &Path,
    dataset_of_interest: &Path,
) -> HttmResult<RelativePathAndSnapMounts> {
    // building our relative path by removing parent below the snap dir
    //
    // for native searches the prefix is are the dirs below the most proximate dataset
    // for user specified dirs/aliases these are specified by the user
    let relative_path = get_relative_path(config, pathdata, proximate_dataset_mount)?;

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

    Ok(RelativePathAndSnapMounts {
        relative_path,
        snap_mounts,
    })
}

fn get_relative_path(
    config: &Config,
    pathdata: &PathData,
    proximate_dataset_mount: &Path,
) -> HttmResult<PathBuf> {
    fn default_path_strip(
        pathdata: &PathData,
        proximate_dataset_mount: &Path,
    ) -> HttmResult<PathBuf> {
        pathdata
            .path_buf
            .strip_prefix(&proximate_dataset_mount)
            .map(|path| path.to_path_buf())
            .map_err(|err| err.into())
    }

    match &config.dataset_collection.opt_map_of_aliases {
        Some(map_of_aliases) => {
            let opt_aliased_local_dir = map_of_aliases
                .iter()
                // do a search for a key with a value
                .find_map(|(local_dir, alias_info)| {
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
                    Ok(alias_stripped_path) => Ok(alias_stripped_path.to_path_buf()),
                    Err(_) => default_path_strip(pathdata, proximate_dataset_mount),
                },
                None => default_path_strip(pathdata, proximate_dataset_mount),
            }
        }
        None => default_path_strip(pathdata, proximate_dataset_mount),
    }
}

fn get_alias_dataset(pathdata: &PathData, map_of_alias: &MapOfAliases) -> Option<PathBuf> {
    // find_map_first should return the first seq result with a par_iter
    // but not with a par_bridge
    pathdata.path_buf.ancestors().find_map(|ancestor| {
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
    pathdata
        .path_buf
        .ancestors()
        .find_map(|ancestor| {
            if map_of_datasets.contains_key(ancestor) {
                Some(ancestor.to_path_buf())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            HttmError::new(
                "httm could not identify any qualifying dataset.  \
                Maybe consider specifying manually at SNAP_POINT?",
            )
            .into()
        })
}

fn get_versions(search_bundle: &RelativePathAndSnapMounts) -> HttmResult<Vec<PathData>> {
    // get the DirEntry for our snapshot path which will have all our possible
    // snapshots, like so: .zfs/snapshots/<some snap name>/
    //
    // BTreeMap will then remove duplicates with the same system modify time and size/file len
    let unique_versions: BTreeMap<(SystemTime, u64), PathData> = search_bundle
        .snap_mounts
        .par_iter()
        .map(|path| path.join(&search_bundle.relative_path))
        .map(|joined_path| PathData::from(joined_path.as_path()))
        .filter_map(|pathdata| {
            pathdata
                .metadata
                .map(|metadata| ((metadata.modify_time, metadata.size), pathdata))
        })
        .collect();

    let sorted_versions: Vec<PathData> = unique_versions.into_values().collect();

    Ok(sorted_versions)
}
