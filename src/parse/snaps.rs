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

use rayon::prelude::*;
use which::which;

use crate::data::system_map::{FilesystemType, MountType, VecOfSnaps};
use crate::library::utility::{HttmError, HttmResult};
use crate::{
    MapOfDatasets, MapOfSnaps, BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX,
    ZFS_SNAPSHOT_DIRECTORY,
};

// fans out precompute of snap mounts to the appropriate function based on fstype
pub fn precompute_snap_mounts(map_of_datasets: &MapOfDatasets) -> HttmResult<MapOfSnaps> {
    let opt_root_mount_path: Option<&PathBuf> =
        map_of_datasets
            .par_iter()
            .find_map_first(|(mount, dataset_info)| match dataset_info.fs_type {
                FilesystemType::Btrfs => {
                    if dataset_info.name.as_str() == "/" {
                        Some(mount)
                    } else {
                        None
                    }
                }
                FilesystemType::Zfs => None,
            });

    let map_of_snaps: MapOfSnaps = map_of_datasets
        .par_iter()
        .flat_map(|(mount, dataset_info)| {
            let snap_mounts = match dataset_info.fs_type {
                FilesystemType::Zfs => precompute_from_defined_mounts(mount, &dataset_info.fs_type),
                FilesystemType::Btrfs => match opt_root_mount_path {
                    Some(root_mount_path) => match dataset_info.mount_type {
                        MountType::Local => precompute_from_btrfs_cmd(mount, root_mount_path),
                        MountType::Network => {
                            precompute_from_defined_mounts(mount, &dataset_info.fs_type)
                        }
                    },
                    None => precompute_from_defined_mounts(mount, &dataset_info.fs_type),
                },
            };

            snap_mounts.map(|snap_mounts| (mount.clone(), snap_mounts))
        })
        .collect();

    if map_of_snaps.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok(map_of_snaps)
    }
}

// build paths to all snap mounts
fn precompute_from_btrfs_cmd(
    mount_point_path: &Path,
    root_mount_path: &Path,
) -> HttmResult<VecOfSnaps> {
    fn parse(
        mount_point_path: &Path,
        root_mount_path: &Path,
        btrfs_command: &Path,
    ) -> HttmResult<VecOfSnaps> {
        let exec_command = btrfs_command;
        let arg_path = mount_point_path.to_string_lossy();
        let args = vec!["subvolume", "list", "-a", "-s", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        let snaps = command_output
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

        Ok(snaps)
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

// similar to btrfs precompute, build paths to all snap mounts for zfs (all) and btrfs snapper (for networked datasets only)
fn precompute_from_defined_mounts(
    mount_point_path: &Path,
    fs_type: &FilesystemType,
) -> HttmResult<VecOfSnaps> {
    let snaps = match fs_type {
        FilesystemType::Btrfs => read_dir(mount_point_path.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY))?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path().join(BTRFS_SNAPPER_SUFFIX))
            .collect(),
        FilesystemType::Zfs => read_dir(mount_point_path.join(ZFS_SNAPSHOT_DIRECTORY))?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path())
            .collect(),
    };

    Ok(snaps)
}
