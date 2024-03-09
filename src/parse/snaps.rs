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
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::user_has_effective_root;
use crate::parse::mounts::PROC_MOUNTS;
use crate::parse::mounts::{DatasetMetadata, FilesystemType, MountType};
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX, ROOT_DIRECTORY, TM_DIR_LOCAL,
    TM_DIR_REMOTE, ZFS_SNAPSHOT_DIRECTORY,
};
use hashbrown::HashMap;
use once_cell::sync::OnceCell;
use proc_mounts::MountIter;
use rayon::prelude::*;
use std::fs::read_dir;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use which::which;

static BTRFS_ROOT: OnceCell<PathBuf> = OnceCell::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfSnaps {
    inner: HashMap<PathBuf, Vec<PathBuf>>,
}

impl From<HashMap<PathBuf, Vec<PathBuf>>> for MapOfSnaps {
    fn from(map: HashMap<PathBuf, Vec<PathBuf>>) -> Self {
        Self { inner: map }
    }
}

impl Deref for MapOfSnaps {
    type Target = HashMap<PathBuf, Vec<PathBuf>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MapOfSnaps {
    // fans out precompute of snap mounts to the appropriate function based on fstype
    pub fn new(map_of_datasets: &HashMap<PathBuf, DatasetMetadata>) -> HttmResult<Self> {
        let map_of_snaps: HashMap<PathBuf, Vec<PathBuf>> = map_of_datasets
            .par_iter()
            .map(|(mount, dataset_info)| {
                let snap_mounts: HttmResult<Vec<PathBuf>> = match dataset_info.fs_type {
                    FilesystemType::Zfs | FilesystemType::Nilfs2 | FilesystemType::Apfs => {
                        Self::from_defined_mounts(mount, dataset_info)
                    }
                    FilesystemType::Btrfs => match dataset_info.mount_type {
                        MountType::Network => Self::from_defined_mounts(mount, dataset_info),
                        MountType::Local => {
                            const USER_REQUIRES_ROOT: &str = "User is required to have super user permissions to determine the location of btrfs snapshots.";

                            match user_has_effective_root(&USER_REQUIRES_ROOT) {
                                Ok(_) => Self::from_btrfs_cmd(mount, map_of_datasets),
                                Err(err) => return Err(err),
                            }
                            
                        }
                    },
                };

                snap_mounts.map(|snap_mounts| (mount.clone(), snap_mounts))
            })
            .collect::<HttmResult<_>>()?;

        if map_of_snaps.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(map_of_snaps.into())
        }
    }

    // build paths to all snap mounts
    fn from_btrfs_cmd(
        base_mount: &Path,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
    ) -> HttmResult<Vec<PathBuf>> {
        let btrfs_command = which("btrfs").map_err(|_err| {
            HttmError::new(
                "'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
            )
        })?;

        let exec_command = btrfs_command;
        let arg_path = base_mount.to_string_lossy();
        let args = vec!["subvolume", "show", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        let snaps = command_output
            .split_once("Snapshot(s):\n")
            .map(|(_pre, snap_paths)| {
                snap_paths
                    .lines()
                    .map(|line| line.trim())
                    .map(|line| Path::new(line))
                    .filter_map(|relative| {
                        Self::parse_btrfs_relative_path(relative, base_mount, map_of_datasets)
                    })
                    .collect()
            })
            .ok_or_else(|| {
                let msg = format!("No snaps found for mount: {:?}", base_mount);
                HttmError::new(&msg)
            })?;

        Ok(snaps)
    }

    fn parse_btrfs_relative_path(
        relative: &Path,
        base_mount: &Path,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
    ) -> Option<PathBuf> {
        let mut path_iter = relative.components();

        let opt_dataset = path_iter.next();

        let the_rest = path_iter;

        match opt_dataset
            .and_then(|dataset| {
                map_of_datasets
                    .iter()
                    .find(|(_mount, metadata)| {
                        let needle = dataset.as_os_str().to_string_lossy();
                        let haystack = metadata.source.to_string_lossy();

                        haystack.rfind(needle.as_ref()).is_some()
                    })
                    .map(|(mount, _metadata)| mount)
            })
            .map(|mount| mount.join(the_rest))
        {
            Some(snap_mount) if snap_mount.exists() => {
                return Some(snap_mount);
            }
            _ => {
                let btrfs_root = BTRFS_ROOT.get_or_init(|| {
                    map_of_datasets
                        .iter()
                        .find(|(_mount, metadata)| {
                            metadata.fs_type == FilesystemType::Btrfs
                                && metadata.source.to_string_lossy() == "/"
                        })
                        .map(|(mount, _metadata)| mount.to_owned())
                        .unwrap_or(PathBuf::from(ROOT_DIRECTORY))
                });

                let mut snap_mount = btrfs_root.to_path_buf().join(relative);

                if snap_mount.exists() {
                    return Some(snap_mount);
                }

                snap_mount = base_mount.join(relative);

                if snap_mount.exists() {
                    return Some(snap_mount);
                }

                None
            }
        }
    }

    fn from_defined_mounts(
        mount_point_path: &Path,
        dataset_metadata: &DatasetMetadata,
    ) -> HttmResult<Vec<PathBuf>> {
        let snaps = match dataset_metadata.fs_type {
            FilesystemType::Btrfs => {
                read_dir(mount_point_path.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY))?
                    .flatten()
                    .par_bridge()
                    .map(|entry| entry.path().join(BTRFS_SNAPPER_SUFFIX))
                    .collect()
            }
            FilesystemType::Zfs => read_dir(mount_point_path.join(ZFS_SNAPSHOT_DIRECTORY))?
                .flatten()
                .par_bridge()
                .map(|entry| entry.path())
                .collect(),
            FilesystemType::Apfs => {
                let mut res: Vec<PathBuf> = Vec::new();

                if PathBuf::from(&TM_DIR_LOCAL).exists() {
                    let local = read_dir(TM_DIR_LOCAL)?
                        .par_bridge()
                        .flatten()
                        .flat_map(|entry| read_dir(entry.path()))
                        .flatten_iter()
                        .flatten_iter()
                        .map(|entry| entry.path().join("Data"));

                    res.par_extend(local);
                }

                if PathBuf::from(&TM_DIR_REMOTE).exists() {
                    let remote = read_dir(TM_DIR_REMOTE)?
                        .par_bridge()
                        .flatten()
                        .flat_map(|entry| read_dir(entry.path()))
                        .flatten_iter()
                        .flatten_iter()
                        .map(|entry| entry.path().join(entry.file_name()).join("Data"));

                    res.par_extend(remote);
                }

                res
            }
            FilesystemType::Nilfs2 => {
                let source_path = Path::new(&dataset_metadata.source);

                let mount_iter = MountIter::new_from_file(&*PROC_MOUNTS)?;

                mount_iter
                    .par_bridge()
                    .flatten()
                    .filter(|mount_info| Path::new(&mount_info.source) == source_path)
                    .filter(|mount_info| mount_info.options.iter().any(|opt| opt.contains("cp=")))
                    .map(|mount_info| PathBuf::from(mount_info.dest))
                    .collect()
            }
        };

        Ok(snaps)
    }
}
