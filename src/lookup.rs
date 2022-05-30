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

use crate::{Config, HttmError, PathData, SnapPoint, ZFS_HIDDEN_SNAPSHOT_DIRECTORY};

#[derive(Debug, Clone)]
pub enum NativeDatasetType {
    MostImmediate,
    AltReplicated,
}

#[derive(Debug, Clone)]
pub struct SearchDirs {
    pub hidden_snapshot_dir: PathBuf,
    pub relative_path: PathBuf,
}

pub fn get_versions(
    config: &Config,
    pathdata: &Vec<PathData>,
) -> Result<[Vec<PathData>; 2], Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prepare for local and replicated backups on alt replicated sets if necessary
    let selected_datasets = if config.opt_alt_replicated {
        vec![
            NativeDatasetType::AltReplicated,
            NativeDatasetType::MostImmediate,
        ]
    } else {
        vec![NativeDatasetType::MostImmediate]
    };

    let all_snap_versions: Vec<PathData> =
        get_all_snap_versions(config, pathdata, &selected_datasets)?;

    // create vec of live copies - unless user doesn't want it!
    let live_versions: Vec<PathData> = if !config.opt_no_live_vers {
        pathdata.to_owned()
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
    pathdata: &Vec<PathData>,
    selected_datasets: &Vec<NativeDatasetType>,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // create vec of all local and replicated backups at once
    let all_snap_versions: Vec<PathData> = pathdata
        .par_iter()
        .map(|path_data| {
            selected_datasets
                .par_iter()
                .map(|dataset_type| get_search_dirs(config, path_data, dataset_type))
                .flatten()
        })
        .flatten()
        .flatten()
        .flat_map(|search_dirs| get_versions_per_dataset(&search_dirs))
        .flatten()
        .collect();

    Ok(all_snap_versions)
}

pub fn get_search_dirs(
    config: &Config,
    file_pathdata: &PathData,
    requested_dataset_type: &NativeDatasetType,
) -> Result<Vec<SearchDirs>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // here, we take our file path and get back possibly multiple ZFS dataset mountpoints
    // and our most immediate dataset mount point (which is always the same) for
    // a single file
    //
    // we ask a few questions: has the location been user defined? if not, does
    // the user want all local datasets on the system, including replicated datasets?
    // the most common case is: just use the most immediate dataset mount point as both
    // the dataset of interest and most immediate ZFS dataset
    //
    // why? we need both the dataset of interest and the most immediate dataset because we
    // will compare the most immediate dataset to our our canonical path and the difference
    // between ZFS mount point and the canonical path is the path we will use to search the
    // hidden snapshot dirs
    let dataset_collection: Vec<(PathBuf, PathBuf)> = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => vec![(
            defined_dirs.snap_dir.to_owned(),
            defined_dirs.snap_dir.to_owned(),
        )],
        SnapPoint::Native(mount_collection) => {
            let immediate_dataset_mount = get_immediate_dataset(file_pathdata, mount_collection)?;
            match requested_dataset_type {
                NativeDatasetType::MostImmediate => {
                    vec![(immediate_dataset_mount.clone(), immediate_dataset_mount)]
                }
                NativeDatasetType::AltReplicated => {
                    get_alt_replicated_dataset(&immediate_dataset_mount, mount_collection)?
                }
            }
        }
    };

    dataset_collection.par_iter().map( |(dataset_of_interest, immediate_dataset_snap_mount)| {
        // building the snapshot path from our dataset
        let hidden_snapshot_dir: PathBuf = dataset_of_interest.join(ZFS_HIDDEN_SNAPSHOT_DIRECTORY);

        // building our relative path by removing parent below the snap dir
        //
        // for native searches the prefix is are the dirs below the most immediate dataset
        // for user specified dirs these are specified by the user
        let relative_path = match &config.snap_point {
            SnapPoint::UserDefined(defined_dirs) => {
                file_pathdata.path_buf
                    .strip_prefix(&defined_dirs.local_dir).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the LOCAL_DIR value."))
            }
            SnapPoint::Native(_) => {
                // this prefix removal is why we always need the immediate dataset name, even when we are searching an alternate replicated filesystem
                file_pathdata.path_buf
                    .strip_prefix(&immediate_dataset_snap_mount).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the SNAP_DIR and LOCAL_DIR values."))   
                }
            }?;

        Ok(
            SearchDirs {
                hidden_snapshot_dir,
                relative_path: relative_path.to_path_buf(),
            }
        )

    }).collect()
}

fn get_immediate_dataset(
    pathdata: &PathData,
    mount_collection: &HashMap<PathBuf, String>,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // only possible None case is "parent is the '/' dir" because
    // of previous work in the Pathdata new method
    let parent_folder = pathdata.path_buf.parent().unwrap_or_else(|| Path::new("/"));

    // prune away most mount points by filtering - parent folder of file must contain relevant dataset
    let potential_mountpoints: Vec<&PathBuf> = mount_collection
        .par_iter()
        .map(|(mount, _dataset)| mount)
        .filter(|line| parent_folder.starts_with(line))
        .collect();

    // do we have any mount points left? if not print error
    if potential_mountpoints.is_empty() {
        let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?";
        return Err(HttmError::new(msg).into());
    };

    // select the best match for us: the longest, as we've already matched on the parent folder
    // so for /usr/bin, we would then prefer /usr/bin to /usr and /
    let best_potential_mountpoint = match potential_mountpoints
        .par_iter()
        .max_by_key(|x| x.as_os_str().len())
    {
        Some(some_bpmp) => PathBuf::from(some_bpmp),
        None => {
            let msg = format!(
                "There is no best match for a ZFS dataset to use for path {:?}. Sorry!/Not sorry?)",
                pathdata.path_buf
            );
            return Err(HttmError::new(&msg).into());
        }
    };

    Ok(best_potential_mountpoint)
}

fn get_alt_replicated_dataset(
    immediate_dataset_mount: &Path,
    mount_collection: &HashMap<PathBuf, String>,
) -> Result<Vec<(PathBuf, PathBuf)>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let immediate_dataset_fs_name = match &mount_collection.get(immediate_dataset_mount) {
        Some(immediate_dataset_fs_name) => immediate_dataset_fs_name.to_string(),
        None => {
            return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
        }
    };

    // find a filesystem that ends with our most local filesystem name
    // but which has a prefix, like a different pool name: rpool might be
    // replicated to tank/rpool
    let mut alt_replicated_mounts: Vec<&PathBuf> = mount_collection
        .par_iter()
        .filter(|(_mount, fs_name)| fs_name != &&immediate_dataset_fs_name)
        .filter(|(_mount, fs_name)| fs_name.ends_with(immediate_dataset_fs_name.as_str()))
        .map(|(mount, _fs_name)| mount)
        .collect();

    if alt_replicated_mounts.is_empty() {
        // could not find the any replicated mounts
        Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
    } else {
        alt_replicated_mounts.sort_unstable_by_key(|path| path.as_os_str().len());
        let res = alt_replicated_mounts
            .into_iter()
            .map(|alt_replicated_mount| {
                (
                    alt_replicated_mount.to_owned(),
                    immediate_dataset_mount.to_path_buf(),
                )
            })
            .collect();
        Ok(res)
    }
}

fn get_versions_per_dataset(
    search_dirs: &SearchDirs,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get the DirEntry for our snapshot path which will have all our possible
    // snapshots, like so: .zfs/snapshots/<some snap name>/
    //
    // hashmap will then remove duplicates with the same system modify time and size/file len
    let unique_versions: HashMap<(SystemTime, u64), PathData> =
        read_dir(search_dirs.hidden_snapshot_dir.as_path())?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path())
            .map(|path| path.join(&search_dirs.relative_path))
            .map(|path| PathData::from(path.as_path()))
            .filter(|pathdata| !pathdata.is_phantom)
            .map(|pathdata| ((pathdata.system_time, pathdata.size), pathdata))
            .collect();

    let mut vec_pathdata: Vec<PathData> = unique_versions.into_par_iter().map(|(_, v)| v).collect();

    vec_pathdata.par_sort_unstable_by_key(|pathdata| pathdata.system_time);

    Ok(vec_pathdata)
}
