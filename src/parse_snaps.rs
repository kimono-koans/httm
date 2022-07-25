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

use hashbrown::HashMap;
use rayon::prelude::*;
use which::which;

use crate::utility::HttmError;
use crate::{FilesystemType, HttmResult, ZFS_SNAPSHOT_DIRECTORY};

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
        .flat_map(|(mount, (_dataset, fstype))| {
            let snap_mounts = match fstype {
                FilesystemType::Zfs => precompute_zfs_snap_mounts(mount),
                FilesystemType::Btrfs => match opt_root_mount_path {
                    Some(root_mount_path) => precompute_btrfs_snap_mounts(mount, root_mount_path),
                    None => Err(HttmError::new("No btrfs root mount found on this system.").into()),
                },
            };

            snap_mounts.map(|snap_mounts| (mount.clone(), snap_mounts))
        })
        .collect();

    map_of_snaps
}

// build paths to all snap mounts
fn precompute_btrfs_snap_mounts(
    mount_point_path: &Path,
    root_mount_path: &Path,
) -> HttmResult<Vec<PathBuf>> {
    fn parse(
        mount_point_path: &Path,
        root_mount_path: &Path,
        btrfs_command: &Path,
    ) -> HttmResult<Vec<PathBuf>> {
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
            "'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
        )
        .into())
    }
}

// similar to btrfs precompute, build paths to all snap mounts
fn precompute_zfs_snap_mounts(mount_point_path: &Path) -> HttmResult<Vec<PathBuf>> {
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
