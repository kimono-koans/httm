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

use std::{collections::BTreeMap, path::Path, path::PathBuf, process::Command as ExecProcess};

use proc_mounts::MountIter;
use rayon::iter::Either;
use rayon::prelude::*;
use which::which;

use crate::data::filesystem_map::{DatasetMetadata, FilesystemType, MountType};
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{get_common_path, get_fs_type_from_hidden_dir};
use crate::parse::snaps::precompute_snap_mounts;
use crate::{
    MapOfDatasets, MapOfSnaps, OptBtrfsCommonSnapDir, VecOfFilterDirs, ZFS_SNAPSHOT_DIRECTORY,
};

pub const ZFS_FSTYPE: &str = "zfs";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";

// divide by the type of system we are on
// Linux allows us the read proc mounts
#[allow(clippy::type_complexity)]
pub fn parse_mounts_exec() -> HttmResult<(MapOfDatasets, MapOfSnaps, VecOfFilterDirs)> {
    let (map_of_datasets, vec_filter_dirs) = if cfg!(target_os = "linux") {
        parse_from_proc_mounts()?
    } else {
        parse_from_mount_cmd()?
    };

    let map_of_snaps = precompute_snap_mounts(&map_of_datasets)?;

    Ok((map_of_datasets, map_of_snaps, vec_filter_dirs))
}

// parsing from proc mounts is both faster and necessary for certain btrfs features
// for instance, allows us to read subvolumes mounts, like "/@" or "/@home"
#[allow(clippy::type_complexity)]
fn parse_from_proc_mounts() -> HttmResult<(MapOfDatasets, VecOfFilterDirs)> {
    let (map_of_datasets, filter_dirs): (MapOfDatasets, Vec<PathBuf>) = MountIter::new()?
        .par_bridge()
        .flatten()
        // but exclude snapshot mounts.  we want only the raw filesystems
        .filter(|mount_info| {
            !mount_info
                .dest
                .to_string_lossy()
                .contains(ZFS_SNAPSHOT_DIRECTORY)
        })
        .partition_map(|mount_info| match &mount_info.fstype.as_str() {
            &ZFS_FSTYPE => Either::Left((
                mount_info.dest,
                DatasetMetadata {
                    name: mount_info.source.to_string_lossy().into_owned(),
                    fs_type: FilesystemType::Zfs,
                    mount_type: MountType::Local,
                },
            )),
            &SMB_FSTYPE | &AFP_FSTYPE | &NFS_FSTYPE => {
                match get_fs_type_from_hidden_dir(&mount_info.dest) {
                    Ok(FilesystemType::Zfs) => Either::Left((
                        mount_info.dest,
                        DatasetMetadata {
                            name: mount_info.source.to_string_lossy().into_owned(),
                            fs_type: FilesystemType::Zfs,
                            mount_type: MountType::Network,
                        },
                    )),
                    Ok(FilesystemType::Btrfs) => Either::Left((
                        mount_info.dest,
                        DatasetMetadata {
                            name: mount_info.source.to_string_lossy().into_owned(),
                            fs_type: FilesystemType::Btrfs,
                            mount_type: MountType::Network,
                        },
                    )),
                    Err(_) => Either::Right(mount_info.dest),
                }
            }
            &BTRFS_FSTYPE => {
                let keyed_options: BTreeMap<String, String> = mount_info
                    .options
                    .par_iter()
                    .filter(|line| line.contains('='))
                    .filter_map(|line| {
                        line.split_once(&"=")
                            .map(|(key, value)| (key.to_owned(), value.to_owned()))
                    })
                    .collect();

                let name = match keyed_options.get("subvol") {
                    Some(subvol) => subvol.clone(),
                    None => mount_info.source.to_string_lossy().into_owned(),
                };

                let fs_type = FilesystemType::Btrfs;

                let mount_type = MountType::Local;

                Either::Left((
                    mount_info.dest,
                    DatasetMetadata {
                        name,
                        fs_type,
                        mount_type,
                    },
                ))
            }
            _ => Either::Right(mount_info.dest),
        });

    if map_of_datasets.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok((map_of_datasets, filter_dirs))
    }
}

// old fashioned parsing for non-Linux systems, nearly as fast, works everywhere with a mount command
// both methods are much faster than using zfs command
#[allow(clippy::type_complexity)]
fn parse_from_mount_cmd() -> HttmResult<(MapOfDatasets, VecOfFilterDirs)> {
    fn parse(mount_command: &Path) -> HttmResult<(MapOfDatasets, VecOfFilterDirs)> {
        let command_output =
            std::str::from_utf8(&ExecProcess::new(mount_command).output()?.stdout)?.to_owned();

        // parse "mount" for filesystems and mountpoints
        let (map_of_datasets, filter_dirs): (MapOfDatasets, Vec<PathBuf>) = command_output
            .par_lines()
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
            .partition_map(|(filesystem, mount)| {
                match get_fs_type_from_hidden_dir(&mount) {
                    Ok(FilesystemType::Zfs) => {
                        Either::Left((mount, DatasetMetadata {
                            name: filesystem,
                            fs_type: FilesystemType::Zfs,
                            mount_type: MountType::Local
                        }))
                    },
                    Ok(FilesystemType::Btrfs) => {
                        Either::Left((mount, DatasetMetadata{
                            name: filesystem,
                            fs_type: FilesystemType::Btrfs,
                            mount_type: MountType::Local
                        }))
                    },
                    Err(_) => {
                        Either::Right(mount)
                    }
                }
            });

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, filter_dirs))
        }
    }

    // do we have the necessary commands for search if user has not defined a snap point?
    // if so run the mount search, if not print some errors
    if let Ok(mount_command) = which("mount") {
        parse(&mount_command)
    } else {
        Err(HttmError::new(
            "'mount' command not be found. Make sure the command 'mount' is in your path.",
        )
        .into())
    }
}

// if we have some btrfs mounts, we check to see if there is a snap directory in common
// so we can hide that common path from searches later
pub fn get_common_snap_dir(
    map_of_datasets: &MapOfDatasets,
    map_of_snaps: &MapOfSnaps,
) -> OptBtrfsCommonSnapDir {
    let btrfs_datasets: Vec<&PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, dataset_info)| dataset_info.fs_type == FilesystemType::Btrfs)
        .map(|(mount, _dataset_info)| mount)
        .collect();

    if !btrfs_datasets.is_empty() {
        let vec_snaps: Vec<&PathBuf> = btrfs_datasets
            .into_par_iter()
            .filter_map(|mount| map_of_snaps.get(mount))
            .flat_map(|snap_info| snap_info)
            .collect();

        get_common_path(vec_snaps)
    } else {
        // since snapshots ZFS reside on multiple datasets
        // never have a common snap path
        None
    }
}
