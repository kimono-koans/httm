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

use crate::{Config, HttmError, PathData};
use rayon::prelude::*;
use which::which;

use fxhash::FxHashMap as HashMap;
use std::{
    path::{Path, PathBuf},
    process::Command as ExecProcess,
    time::SystemTime,
};

pub fn lookup_exec(
    config: &Config,
    path_data: Vec<PathData>,
) -> Result<Vec<Vec<PathData>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // create vec of backups
    let snapshot_versions: Vec<PathData> = path_data
        .par_iter()
        .map(|pathdata| get_versions_set(config, pathdata))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync + 'static>>>()?
        .into_iter()
        .flatten()
        .collect();

    // create vec of live copies
    let live_versions: Vec<PathData> = if !config.opt_no_live_vers {
        path_data
    } else {
        Vec::new()
    };

    // check if all files (snap and live) do not exist, if this is true, then user probably messed up
    // and entered a file that never existed?  Or was on a snapshot that has since been destroyed?
    if snapshot_versions.is_empty() && live_versions.iter().all(|i| i.is_phantom) {
        return Err(HttmError::new(
            "Neither a live copy, nor a snapshot copy of such a file appears to exist, so, umm, 🤷? Please try another file.",
        )
        .into());
    }

    // return a vec of vecs with no live copies if that is the user's want
    if live_versions.is_empty() {
        Ok(vec![snapshot_versions])
    } else {
        Ok(vec![snapshot_versions, live_versions])
    }
}

fn get_versions_set(
    config: &Config,
    pathdata: &PathData,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let dataset = if let Some(ref snap_point) = config.opt_snap_point {
        snap_point.to_owned()
    } else {
        get_dataset(pathdata)?
    };
    get_versions(config, pathdata, dataset)
}

fn get_versions(
    config: &Config,
    pathdata: &PathData,
    dataset: PathBuf,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // building the snapshot path
    let snapshot_dir: PathBuf = [&dataset.to_string_lossy(), ".zfs", "snapshot"]
        .iter()
        .collect();

    // building our local relative path by removing parent
    // directories below the remote/snap mount point
    //
    // TODO: I *could* step backwards and check each ancestor folder for .zfs dirs as
    // an auto detect mechanism.  Currently, we rely on the user to provide.
    //
    // It would only work on ZFS datasets and not local-rsync-ed sets. :(
    // Presently, defaults to everything below the working dir in the unspecified case.
    let local_path = if config.opt_snap_point.is_some() {
        pathdata.path_buf
        .strip_prefix(&config.opt_local_dir).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the LOCAL_DIR value."))
    } else {
        pathdata.path_buf
        .strip_prefix(&dataset).map_err(|_| HttmError::new("Are you sure you're in the correct working directory?  Perhaps you need to set the SNAP_DIR and LOCAL_DIR values."))
    }?;

    // get the DirEntry for our snapshot path which will have all our possible
    // needed snapshots
    let versions = std::fs::read_dir(snapshot_dir)?
        .into_iter()
        .flatten()
        .par_bridge()
        .map(|entry| entry.path())
        .map(|path| path.join(local_path))
        .map(|path| PathData::new(config, &path))
        .filter(|pathdata| !pathdata.is_phantom)
        .collect::<Vec<PathData>>();

    // filter here will remove all the None values silently as we build a list of unique versions
    // and our hashmap will then remove duplicates with the same system modify time and size/file len
    let mut unique_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();
    let _ = versions.into_iter().for_each(|pathdata| {
        let _ = unique_versions.insert((pathdata.system_time, pathdata.size), pathdata);
    });

    let mut sorted: Vec<_> = unique_versions.into_iter().collect();

    sorted.par_sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}

fn get_dataset(
    pathdata: &PathData,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let path = &pathdata.path_buf;

    // method parent() cannot return None, when path is an absolute path and not the root dir
    // and this should have been previous set in PathData new(), so None only if the root dir
    let parent_folder = path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_string_lossy();

    // ingest datasets from the cmdline
    let shell = which("sh")
        .map_err(|_| {
            HttmError::new("sh command not found. Make sure the command 'sh' is in your path.")
        })?
        .to_string_lossy()
        .to_string();

    let exec_args = " list -H -t filesystem -o mountpoint,mounted";
    let exec_command = which("zfs")
        .map_err(|_| {
            HttmError::new("zfs command not found. Make sure the command 'zfs' is in your path.")
        })?
        .to_string_lossy()
        .to_string()
        + exec_args;

    let datasets_from_zfs = std::str::from_utf8(
        &ExecProcess::new(shell)
            .arg("-c")
            .arg(exec_command)
            .output()?
            .stdout,
    )?
    .to_owned();

    // prune most datasets by match the parent_folder of file contains those datasets
    let select_potential_mountpoints = datasets_from_zfs
        .par_lines()
        .filter(|line| line.contains("yes"))
        .filter_map(|line| line.split('\t').next())
        .map(|line| line.trim())
        .filter(|line| parent_folder.contains(line))
        .collect::<Vec<&str>>();

    // do we have any left, if yes just continue
    if select_potential_mountpoints.is_empty() {
        let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?";
        return Err(HttmError::new(msg).into());
    };

    // select the best match for us: the longest, as we've already matched on the parent folder
    // so for /usr/bin/bash, we prefer /usr/bin to /usr
    let best_potential_mountpoint = if let Some(some_bpmp) = select_potential_mountpoints
        .par_iter()
        .max_by_key(|x| x.len())
    {
        some_bpmp
    } else {
        let msg = format!(
            "There is no best match for a ZFS dataset to use for path {:?}. Sorry!/Not sorry?)",
            path
        );
        return Err(HttmError::new(&msg).into());
    };

    Ok(PathBuf::from(best_potential_mountpoint))
}
