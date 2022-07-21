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

use std::{path::Path, path::PathBuf, process::Command as ExecProcess};

use proc_mounts::MountIter;
use rayon::iter::Either;
use rayon::prelude::*;
use which::which;

use crate::parse_snaps::precompute_snap_mounts;
use crate::utility::{get_common_path, get_fs_type_from_hidden_dir, HttmError};
use crate::{
    AHashMap as HashMap, FilesystemType, HttmResult, AFP_FSTYPE, BTRFS_FSTYPE, NFS_FSTYPE,
    SMB_FSTYPE, ZFS_FSTYPE, ZFS_SNAPSHOT_DIRECTORY,
};

// divide by the type of system we are on
// Linux allows us the read proc mounts
#[allow(clippy::type_complexity)]
pub fn parse_mounts_exec() -> HttmResult<(
    HashMap<PathBuf, (String, FilesystemType)>,
    HashMap<PathBuf, Vec<PathBuf>>,
    Vec<PathBuf>,
)> {
    let (map_of_datasets, vec_filter_dirs) = if cfg!(target_os = "linux") {
        parse_from_proc_mounts()?
    } else {
        parse_from_mount_cmd()?
    };

    let map_of_snaps = precompute_snap_mounts(&map_of_datasets);

    Ok((map_of_datasets, map_of_snaps, vec_filter_dirs))
}

// parsing from proc mounts is both faster and necessary for certain btrfs features
// for instance, allows us to read subvolumes mounts, like "/@" or "/@home"
#[allow(clippy::type_complexity)]
fn parse_from_proc_mounts() -> HttmResult<(HashMap<PathBuf, (String, FilesystemType)>, Vec<PathBuf>)>
{
    let (map_of_datasets, vec_of_filter_dirs): (
        HashMap<PathBuf, (String, FilesystemType)>,
        Vec<PathBuf>,
    ) = MountIter::new()?
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
                (
                    mount_info.source.to_string_lossy().into_owned(),
                    FilesystemType::Zfs,
                ),
            )),
            &SMB_FSTYPE | &AFP_FSTYPE | &NFS_FSTYPE => {
                match get_fs_type_from_hidden_dir(&mount_info.dest) {
                    Ok(FilesystemType::Zfs) => Either::Left((
                        mount_info.dest,
                        (
                            mount_info.source.to_string_lossy().into_owned(),
                            FilesystemType::Zfs,
                        ),
                    )),
                    Ok(FilesystemType::Btrfs) => Either::Left((
                        mount_info.dest,
                        (
                            mount_info.source.to_string_lossy().into_owned(),
                            FilesystemType::Btrfs,
                        ),
                    )),
                    Err(_) => Either::Right(mount_info.dest),
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
                    None => mount_info.source.to_string_lossy().into_owned(),
                };

                let fstype = FilesystemType::Btrfs;

                Either::Left((mount_info.dest, (subvol, fstype)))
            }
            _ => Either::Right(mount_info.dest),
        });

    if map_of_datasets.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok((map_of_datasets, vec_of_filter_dirs))
    }
}

// old fashioned parsing for non-Linux systems, nearly as fast, works everywhere with a mount command
// both methods are much faster than using zfs command
#[allow(clippy::type_complexity)]
fn parse_from_mount_cmd() -> HttmResult<(HashMap<PathBuf, (String, FilesystemType)>, Vec<PathBuf>)>
{
    fn parse(
        mount_command: &Path,
    ) -> HttmResult<(HashMap<PathBuf, (String, FilesystemType)>, Vec<PathBuf>)> {
        let command_output =
            std::str::from_utf8(&ExecProcess::new(mount_command).output()?.stdout)?.to_owned();

        // parse "mount" for filesystems and mountpoints
        let (map_of_datasets, vec_of_filter_dirs): (
            HashMap<PathBuf, (String, FilesystemType)>,
            Vec<PathBuf>,
        ) = command_output
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
                        Either::Left((mount, (filesystem, FilesystemType::Zfs)))
                    },
                    Ok(FilesystemType::Btrfs) => {
                        Either::Left((mount, (filesystem, FilesystemType::Btrfs)))
                    },
                    Err(_) => {
                        Either::Right(mount)
                    }
                }
            });

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, vec_of_filter_dirs))
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
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
    map_of_snaps: &HashMap<PathBuf, Vec<PathBuf>>,
) -> Option<PathBuf> {
    let btrfs_datasets: Vec<&PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, (_dataset, fstype))| fstype == &FilesystemType::Btrfs)
        .map(|(mount, (_dataset, _fstype))| mount)
        .collect();

    if !btrfs_datasets.is_empty() {
        let vec_snaps: Vec<&PathBuf> = btrfs_datasets
            .into_par_iter()
            .map(|mount| map_of_snaps.get(mount))
            .flatten()
            .flatten()
            .collect();

        get_common_path(vec_snaps)
    } else {
        // since snapshots ZFS reside on multiple datasets
        // never have a common snap path
        None
    }
}
