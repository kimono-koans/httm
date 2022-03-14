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

use crate::Config;
use crate::HttmError;
use crate::PathData;

use fxhash::FxHashMap as HashMap;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command as ExecProcess,
    time::SystemTime,
};

pub fn run_search(
    config: &Config,
    path_data: Vec<Option<PathData>>,
) -> Result<Vec<Vec<PathData>>, Box<dyn std::error::Error>> {
    // create vec of backups
    let mut snapshot_versions: Vec<PathData> = Vec::new();

    for instance_pd in path_data.iter().flatten() {
        let dataset = if let Some(ref snap_point) = config.opt_snap_point {
            snap_point.to_owned()
        } else {
            get_dataset(instance_pd)?
        };

        snapshot_versions.extend_from_slice(&get_versions(config, instance_pd, dataset)?)
    }

    // create vec of live copies
    let live_versions: Vec<PathData> = if !config.opt_no_live_vers {
        path_data.into_iter().flatten().collect()
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

    if live_versions.is_empty() {
        Ok(vec![snapshot_versions])
    } else {
        Ok(vec![snapshot_versions, live_versions])
    }
}

fn get_versions(
    config: &Config,
    pathdata: &PathData,
    dataset: OsString,
) -> Result<Vec<PathData>, Box<dyn std::error::Error>> {
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
        if let Some(local_dir) = &config.opt_local_dir {
            pathdata.path_buf.strip_prefix(local_dir)?
        } else {
            match pathdata.path_buf.strip_prefix(&config.current_working_dir) {
                Ok(path) => path,
                Err(_) => {
                    let msg = "Are you sure you're in the correct working directory?  Perhaps you need to set the RELATIVE_DIR value.".to_string();
                    return Err(HttmError::new(&msg).into());
                }
            }
        }
    } else {
        pathdata.path_buf.strip_prefix(&dataset)?
    };

    let snapshots = std::fs::read_dir(snapshot_dir)?;

    let versions: Vec<_> = snapshots
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .map(|path| path.join(local_path))
        .collect();

    let mut unique_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();

    for path in &versions {
        if let Some(pd) = PathData::new(path) {
            if !pd.is_phantom {
                unique_versions.insert((pd.system_time, pd.size), pd);
            }
        }
    }

    let mut sorted: Vec<_> = unique_versions.into_iter().collect();

    sorted.sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}

fn get_dataset(pathdata: &PathData) -> Result<OsString, Box<dyn std::error::Error>> {
    let path = &pathdata.path_buf;

    // method parent() cannot return None, when path is a canonical path
    // and this should have been previous set in PathData new(), so None only if the root dir
    let parent_folder = path
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_string_lossy();

    // ingest datasets from the cmdline
    let exec_command = "zfs list -H -t filesystem -o mountpoint,mounted";
    let datasets_from_zfs = std::str::from_utf8(
        &ExecProcess::new("env")
            .arg("sh")
            .arg("-c")
            .arg(exec_command)
            .output()?
            .stdout,
    )?
    .to_owned();

    // prune most datasets by match the parent_folder of file contains those datasets
    let select_potential_mountpoints = datasets_from_zfs
        .lines()
        .filter(|line| line.contains("yes"))
        .filter_map(|line| line.split('\t').next())
        .map(|line| line.trim())
        .filter(|line| parent_folder.contains(line))
        .collect::<Vec<&str>>();

    // do we have any left, if yes just continue
    if select_potential_mountpoints.is_empty() {
        let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually at SNAP_POINT?"
            .to_string();
        return Err(HttmError::new(&msg).into());
    };

    // select the best match for us: the longest, as we've already matched on the parent folder
    // so for /usr/bin/bash, we prefer /usr/bin to /usr
    let best_potential_mountpoint = if let Some(some_bpmp) = select_potential_mountpoints
        .clone()
        .into_iter()
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

    Ok(OsString::from(best_potential_mountpoint))
}
