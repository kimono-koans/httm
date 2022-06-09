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

use std::{fs::read_dir, path::Path, path::PathBuf, process::Command as ExecProcess};

use fxhash::FxHashMap as HashMap;
use proc_mounts::MountIter;
use rayon::prelude::*;
use which::which;

use crate::versions_lookup::get_alt_replicated_datasets;
use crate::{
    FilesystemType, HttmError, BTRFS_FSTYPE, ZFS_FSTYPE,
    ZFS_SNAPSHOT_DIRECTORY,
};

#[allow(clippy::type_complexity)]
pub fn get_filesystems_list() -> Result<
    (
        HashMap<PathBuf, (String, FilesystemType)>,
        Option<HashMap<PathBuf, Vec<PathBuf>>>,
    ),
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    let (mount_collection, map_of_snaps) = if cfg!(target_os = "linux") {
        parse_from_proc_mounts()?
    } else {
        (parse_from_mount_cmd()?, None)
    };

    Ok((mount_collection, map_of_snaps))
}

// both faster and necessary for certain btrfs features
// allows us to read subvolumes
#[allow(clippy::type_complexity)]
fn parse_from_proc_mounts() -> Result<
    (
        HashMap<PathBuf, (String, FilesystemType)>,
        Option<HashMap<PathBuf, Vec<PathBuf>>>,
    ),
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    let mount_collection: HashMap<PathBuf, (String, FilesystemType)> = MountIter::new()?
        .into_iter()
        .par_bridge()
        .flatten()
        .filter(|mount_info| {
            mount_info.fstype.contains(BTRFS_FSTYPE) || mount_info.fstype.contains(ZFS_FSTYPE)
        })
        // but exclude snapshot mounts.  we want the raw filesystem names.
        .filter(|mount_info| {
            !mount_info
                .dest
                .to_string_lossy()
                .contains(ZFS_SNAPSHOT_DIRECTORY)
        })
        .map(|mount_info| match &mount_info.fstype {
            fs if fs == ZFS_FSTYPE => (
                mount_info.dest,
                (
                    mount_info.source.to_string_lossy().to_string(),
                    FilesystemType::Zfs,
                ),
            ),
            fs if fs == BTRFS_FSTYPE => {
                let keyed_options: HashMap<String, String> = mount_info
                    .options
                    .par_iter()
                    .filter(|line| line.contains('='))
                    .filter_map(|line| {
                        line.split_once(&"=")
                            .map(|(key, value)| (key.to_owned(), value.to_owned()))
                    })
                    .collect();

                let subvol = match keyed_options.get("subvol") {
                    Some(subvol) => subvol.to_owned(),
                    None => mount_info.source.to_string_lossy().to_string(),
                };

                let fstype = FilesystemType::Btrfs;

                (mount_info.dest, (subvol, fstype))
            }
            _ => unreachable!(),
        })
        .filter(|(mount, (_dataset, _fstype))| mount.exists())
        .collect();

    let map_of_snaps = precompute_snap_mounts(&mount_collection).ok();

    if mount_collection.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok((mount_collection, map_of_snaps))
    }
}

pub fn precompute_snap_mounts(
    mount_collection: &HashMap<PathBuf, (String, FilesystemType)>,
) -> Result<HashMap<PathBuf, Vec<PathBuf>>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let map_of_snaps = mount_collection
        .par_iter()
        .filter_map(|(mount, (_dataset, fstype))| {
            let snap_mounts = match fstype {
                FilesystemType::Zfs => precompute_zfs_snap_mounts(mount),
                FilesystemType::Btrfs => precompute_btrfs_snap_mounts(mount),
            };

            match snap_mounts {
                Ok(snap_mounts) => Some((mount.to_owned(), snap_mounts)),
                Err(_) => None,
            }
        })
        .collect();

    Ok(map_of_snaps)
}

fn parse_from_mount_cmd() -> Result<
    HashMap<PathBuf, (String, FilesystemType)>,
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    // read datasets from 'mount' if possible -- this is much faster than using zfs command
    // but I trust we've parsed it correctly less, because BSD and Linux output are different
    fn get_filesystems_and_mountpoints(
        mount_command: &PathBuf,
    ) -> Result<
        HashMap<PathBuf, (String, FilesystemType)>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let command_output =
            std::str::from_utf8(&ExecProcess::new(mount_command).output()?.stdout)?.to_owned();

        // parse "mount" for filesystems and mountpoints
        let mount_collection: HashMap<PathBuf, (String, FilesystemType)> = command_output
            .par_lines()
            // want zfs 
            .filter(|line| line.contains(ZFS_FSTYPE))
            // but exclude snapshot mounts.  we want the raw filesystem names.
            .filter(|line| !line.contains(ZFS_SNAPSHOT_DIRECTORY))
            .filter_map(|line|
                // GNU Linux mount output
                if line.contains("type") {
                    line.split_once(&" type")
                // Busybox and BSD mount output
                } else {
                    line.split_once(&" (")
                }
            )
            .map(|(filesystem_and_mount,_)| filesystem_and_mount )
            .filter_map(|filesystem_and_mount| filesystem_and_mount.split_once(&" on "))
            // sanity check: does the filesystem exist? if not, filter it out
            .map(|(filesystem, mount)| (filesystem.to_owned(), PathBuf::from(mount)))
            .filter(|(_filesystem, mount)| mount.exists())
            .map(|(filesystem, mount)| (mount, (filesystem, FilesystemType::Zfs)))
            .collect();

        if mount_collection.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(mount_collection)
        }
    }

    // do we have the necessary commands for search if user has not defined a snap point?
    // if so run the mount search, if not print some errors
    if let Ok(mount_command) = which("mount") {
        get_filesystems_and_mountpoints(&mount_command)
    } else {
        Err(HttmError::new(
            "mount command not found. Make sure the command 'mount' is in your path.",
        )
        .into())
    }
}

pub fn precompute_alt_replicated(
    mount_collection: &HashMap<PathBuf, (String, FilesystemType)>,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    mount_collection
        .par_iter()
        .filter_map(|(mount, (_dataset, _fstype))| {
            get_alt_replicated_datasets(mount, mount_collection).ok()
        })
        .map(|dataset_collection| {
            (
                dataset_collection.immediate_dataset_mount,
                dataset_collection.datasets_of_interest,
            )
        })
        .collect()
}

pub fn precompute_btrfs_snap_mounts(
    mount_point_path: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // read datasets from 'mount' if possible -- this is much faster than using zfs command
    // but I trust we've parsed it correctly less, because BSD and Linux output are different
    fn parse(
        mount_point_path: &Path,
        btrfs_command: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let exec_command = btrfs_command;
        let arg_path = mount_point_path.to_string_lossy();
        let args = vec!["subvolume", "list", "-s", &arg_path];

        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        // parse "mount" for filesystems and mountpoints
        let snapshot_locations: Vec<PathBuf> = command_output
            .par_lines()
            .filter_map(|line| line.split_once(&"path "))
            .map(|(_first, last)| last)
            .map(|snapshot_location| {
                let snap_path = Path::new(snapshot_location);
                if snap_path.is_absolute() {
                    snap_path.to_path_buf()
                } else {
                    mount_point_path.to_path_buf().join(snap_path)
                }
            })
            .filter(|snapshot_location| snapshot_location.exists())
            .collect();

        if snapshot_locations.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(snapshot_locations)
        }
    }

    if let Ok(btrfs_command) = which("btrfs") {
        let snapshot_locations = parse(mount_point_path, &btrfs_command)?;
        Ok(snapshot_locations)
    } else {
        Err(HttmError::new(
            "btrfs command not found. Make sure the command 'btrfs' is in your path.",
        )
        .into())
    }
}

pub fn precompute_zfs_snap_mounts(
    mount_point_path: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let snap_path = mount_point_path.join(ZFS_SNAPSHOT_DIRECTORY);

    let snapshot_locations: Vec<PathBuf> = read_dir(snap_path)?
        .flatten()
        .par_bridge()
        .map(|entry| entry.path())
        .collect();

    if snapshot_locations.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok(snapshot_locations)
    }
}
