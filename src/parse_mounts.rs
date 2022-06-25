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

use crate::utility::get_common_path;
use crate::versions_lookup::get_alt_replicated_datasets;
use crate::{
    FilesystemType, HttmError, AFP_FSTYPE, BTRFS_FSTYPE, BTRFS_SNAPPER_HIDDEN_DIRECTORY,
    NFS_FSTYPE, SMB_FSTYPE, ZFS_FSTYPE, ZFS_SNAPSHOT_DIRECTORY,
};

// divide by the type of system we are on
// Linux allows us the read proc mounts
#[allow(clippy::type_complexity)]
pub fn get_filesystems_list() -> Result<
    (
        HashMap<PathBuf, (String, FilesystemType)>,
        HashMap<PathBuf, Vec<PathBuf>>,
    ),
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    let (map_of_datasets, map_of_snaps) = if cfg!(target_os = "linux") {
        parse_from_proc_mounts()?
    } else {
        parse_from_mount_cmd()?
    };

    Ok((map_of_datasets, map_of_snaps))
}

// parsing from proc mounts is both faster and necessary for certain btrfs features
// for instance, allows us to read subvolumes mounts, like "/@" or "/@home"
#[allow(clippy::type_complexity)]
fn parse_from_proc_mounts() -> Result<
    (
        HashMap<PathBuf, (String, FilesystemType)>,
        HashMap<PathBuf, Vec<PathBuf>>,
    ),
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    let map_of_datasets: HashMap<PathBuf, (String, FilesystemType)> = MountIter::new()?
        .into_iter()
        .par_bridge()
        .flatten()
        // but exclude snapshot mounts.  we want only the raw filesystems
        .filter(|mount_info| {
            !mount_info
                .dest
                .to_string_lossy()
                .contains(ZFS_SNAPSHOT_DIRECTORY)
        })
        .filter_map(|mount_info| match &mount_info.fstype.as_str() {
            &ZFS_FSTYPE => Some((
                mount_info.dest,
                (
                    mount_info.source.to_string_lossy().to_string(),
                    FilesystemType::Zfs,
                ),
            )),
            &SMB_FSTYPE | &AFP_FSTYPE | &NFS_FSTYPE => {
                if mount_info.dest.join(ZFS_SNAPSHOT_DIRECTORY).exists() {
                    Some((
                        mount_info.dest,
                        (
                            mount_info.source.to_string_lossy().to_string(),
                            FilesystemType::Zfs,
                        ),
                    ))
                } else if mount_info
                    .dest
                    .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
                    .exists()
                {
                    Some((
                        mount_info.dest,
                        (
                            mount_info.source.to_string_lossy().to_string(),
                            FilesystemType::Btrfs,
                        ),
                    ))
                } else {
                    None
                }
            }
            &BTRFS_FSTYPE => {
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
                    Some(subvol) => subvol.clone(),
                    None => mount_info.source.to_string_lossy().to_string(),
                };

                let fstype = FilesystemType::Btrfs;

                Some((mount_info.dest, (subvol, fstype)))
            }
            _ => None,
        })
        .filter(|(mount, (_dataset, _fstype))| mount.exists())
        .collect();

    let map_of_snaps = precompute_snap_mounts(&map_of_datasets);

    if map_of_datasets.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok((map_of_datasets, map_of_snaps))
    }
}

// fans out precompute of snap mounts to the appropriate function based on fstype
pub fn precompute_snap_mounts(
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    let opt_root_mount_path: Option<&PathBuf> =
        map_of_datasets
            .par_iter()
            .find_map_first(|(mount, (dataset, fstype))| match fstype {
                FilesystemType::Btrfs => {
                    if dataset.as_str() == "/" {
                        Some(mount)
                    } else {
                        None
                    }
                }
                FilesystemType::Zfs => None,
            });

    let map_of_snaps: HashMap<PathBuf, Vec<PathBuf>> = map_of_datasets
        .par_iter()
        .filter_map(|(mount, (_dataset, fstype))| {
            let snap_mounts = match fstype {
                FilesystemType::Zfs => precompute_zfs_snap_mounts(mount).ok(),
                FilesystemType::Btrfs => match opt_root_mount_path {
                    Some(root_mount_path) => {
                        precompute_btrfs_snap_mounts(mount, root_mount_path).ok()
                    }
                    None => None,
                },
            };

            snap_mounts.map(|snap_mounts| (mount.clone(), snap_mounts))
        })
        .collect();

    map_of_snaps
}

// old fashioned parsing for non-Linux systems, nearly as fast, works everywhere with a mount command
// both methods are much faster than using zfs command
#[allow(clippy::type_complexity)]
fn parse_from_mount_cmd() -> Result<
    (
        HashMap<PathBuf, (String, FilesystemType)>,
        HashMap<PathBuf, Vec<PathBuf>>,
    ),
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    fn parse(
        mount_command: &PathBuf,
    ) -> Result<
        (
            HashMap<PathBuf, (String, FilesystemType)>,
            HashMap<PathBuf, Vec<PathBuf>>,
        ),
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let command_output =
            std::str::from_utf8(&ExecProcess::new(mount_command).output()?.stdout)?.to_owned();

        // parse "mount" for filesystems and mountpoints
        let map_of_datasets: HashMap<PathBuf, (String, FilesystemType)> = command_output
            .par_lines()
            // want zfs or network datasets which we can auto detest as ZFS
            .filter(|line| {
                line.contains(ZFS_FSTYPE) ||
                line.contains(SMB_FSTYPE) ||
                line.contains(AFP_FSTYPE) ||
                line.contains(NFS_FSTYPE)
            })
            // but exclude snapshot mounts.  we want the raw filesystem names.
            .filter(|line| !line.contains(ZFS_SNAPSHOT_DIRECTORY))
            // where to split, to just have the src and dest of mounts
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
            // mount cmd includes and " on " between src and dest of mount
            .filter_map(|filesystem_and_mount| filesystem_and_mount.split_once(&" on "))
            .map(|(filesystem, mount)| (filesystem.to_owned(), PathBuf::from(mount)))
            // sanity check: does the filesystem exist and have a ZFS hidden dir? if not, filter it out
            // and flip around, mount should key of key/value
            .filter_map(|(filesystem, mount)| {
                if mount.join(ZFS_SNAPSHOT_DIRECTORY).exists() {
                    Some((mount, (filesystem, FilesystemType::Zfs)))
                } else if mount.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY).exists() {
                    Some((mount, (filesystem, FilesystemType::Btrfs)))
                } else {
                    None
                }
            })
            .collect();

        let map_of_snaps = precompute_snap_mounts(&map_of_datasets);

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, map_of_snaps))
        }
    }

    // do we have the necessary commands for search if user has not defined a snap point?
    // if so run the mount search, if not print some errors
    if let Ok(mount_command) = which("mount") {
        parse(&mount_command)
    } else {
        Err(HttmError::new(
            "mount command not found. Make sure the command 'mount' is in your path.",
        )
        .into())
    }
}

// instead of looking up, precompute possible alt replicated mounts before exec
pub fn precompute_alt_replicated(
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    map_of_datasets
        .par_iter()
        .filter_map(|(mount, (_dataset, _fstype))| {
            get_alt_replicated_datasets(mount, map_of_datasets).ok()
        })
        .map(|dataset_collection| {
            (
                dataset_collection.proximate_dataset_mount,
                dataset_collection.datasets_of_interest,
            )
        })
        .collect()
}

// build paths to all snap mounts
pub fn precompute_btrfs_snap_mounts(
    mount_point_path: &Path,
    root_mount_path: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    fn parse(
        mount_point_path: &Path,
        root_mount_path: &Path,
        btrfs_command: &Path,
    ) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let exec_command = btrfs_command;
        let arg_path = mount_point_path.to_string_lossy();
        let args = vec!["subvolume", "list", "-a", "-s", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        let snapshot_locations: Vec<PathBuf> = command_output
            .par_lines()
            .filter_map(|line| line.split_once(&"path "))
            .map(
                |(_first, snap_path)| match snap_path.strip_prefix("<FS_TREE>/") {
                    Some(fs_tree_path) => {
                        // "<FS_TREE>/" should be the root path
                        root_mount_path.join(fs_tree_path)
                    }
                    None => {
                        // btrfs sub list -a -s output includes the sub name (eg @home)
                        // when that sub could be mounted anywhere, so we remove here
                        let snap_path_parsed: PathBuf =
                            Path::new(snap_path).components().skip(1).collect();

                        mount_point_path.join(snap_path_parsed)
                    }
                },
            )
            .filter(|snapshot_location| snapshot_location.exists())
            .collect();

        if snapshot_locations.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(snapshot_locations)
        }
    }

    if let Ok(btrfs_command) = which("btrfs") {
        let snapshot_locations = parse(mount_point_path, root_mount_path, &btrfs_command)?;
        Ok(snapshot_locations)
    } else {
        Err(HttmError::new(
            "btrfs command not found. Make sure the command 'btrfs' is in your path.",
        )
        .into())
    }
}

// similar to btrfs precompute, build paths to all snap mounts
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

// ask what type of system are we on: all ZFS or do we have some btrfs mounts
// if we have some btrfs mounts, we check to see if there is a snap directory in common
// so we can hide that common path from searches later
pub fn get_common_snap_dir(
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
    map_of_snaps: &HashMap<PathBuf, Vec<PathBuf>>,
) -> Option<PathBuf> {
    let opt_snapshot_dir = if map_of_datasets
        .par_iter()
        .any(|(_mount, (_dataset, fstype))| fstype == &FilesystemType::Btrfs)
    {
        let vec_snaps: Vec<&PathBuf> = map_of_snaps.values().flatten().collect();
        get_common_path(vec_snaps)
    } else {
        // since snapshots ZFS reside on multiple datasets
        // never have a common snap path
        None
    };

    opt_snapshot_dir
}
