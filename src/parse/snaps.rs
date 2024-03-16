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
use crate::parse::mounts::BTRFS_ROOT_SUBVOL;
use crate::parse::mounts::PROC_MOUNTS;
use crate::parse::mounts::{DatasetMetadata, FilesystemType, MountType};
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX, ROOT_DIRECTORY, TM_DIR_LOCAL,
    TM_DIR_REMOTE, ZFS_SNAPSHOT_DIRECTORY,
};
use hashbrown::HashMap;
use proc_mounts::MountIter;
use rayon::prelude::*;
use std::fs::read_dir;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::sync::Once;
use which::which;

const BTRFS_COMMAND_REQUIRES_ROOT: &str =
    "User must have super user permissions to determine the location of btrfs snapshots";

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
                let snap_mounts: Vec<PathBuf> = match &dataset_info.fs_type {
                    FilesystemType::Zfs | FilesystemType::Nilfs2 | FilesystemType::Apfs => {
                        Self::from_defined_mounts(mount, dataset_info)
                    }
                    FilesystemType::Btrfs(opt_subvol) => match dataset_info.mount_type {
                        MountType::Network => Self::from_defined_mounts(mount, dataset_info),
                        MountType::Local => {
                            Self::from_btrfs_cmd(mount, dataset_info, opt_subvol, map_of_datasets)
                        }
                    },
                };

                (mount.clone(), snap_mounts)
            })
            .collect();

        if map_of_snaps.is_empty() {
            Err(HttmError::new("httm could not find any valid snapshots on the system.").into())
        } else {
            Ok(map_of_snaps.into())
        }
    }

    // build paths to all snap mounts
    fn from_btrfs_cmd(
        base_mount: &Path,
        base_mount_metadata: &DatasetMetadata,
        opt_subvol: &Option<PathBuf>,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
    ) -> Vec<PathBuf> {
        if user_has_effective_root(&BTRFS_COMMAND_REQUIRES_ROOT).is_err() {
            static USER_HAS_ROOT_WARNING: Once = Once::new();

            USER_HAS_ROOT_WARNING.call_once(|| {
                eprintln!("WARN: httm requires root permissions to detect btrfs snapshot mounts.");
            });
            return Vec::new();
        }

        let Ok(btrfs_command) = which("btrfs") else {
            static BTRFS_COMMAND_AVAILABLE_WARNING: Once = Once::new();

            BTRFS_COMMAND_AVAILABLE_WARNING.call_once(|| {
                eprintln!(
                    "WARN: 'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
                );
            });

            return Vec::new();
        };

        let exec_command = btrfs_command;
        let arg_path = base_mount.to_string_lossy();
        let args = vec!["subvolume", "show", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let Some(command_output) = ExecProcess::new(exec_command)
            .args(&args)
            .output()
            .ok()
            .and_then(|output| {
                std::str::from_utf8(&output.stdout)
                    .map(|string| string.to_owned())
                    .ok()
            })
        else {
            static COULD_NOT_OBTAIN_BTRFS_COMMAND_OUTPUT: Once = Once::new();

            COULD_NOT_OBTAIN_BTRFS_COMMAND_OUTPUT.call_once(|| {
                eprintln!("WARN: Could not obtain btrfs command output.",);
            });
            return Vec::new();
        };

        match command_output
            .split_once("Snapshot(s):\n")
            .map(|(_pre, snap_paths)| {
                snap_paths
                    .par_lines()
                    .map(|line| line.trim())
                    .map(|line| Path::new(line))
                    .filter_map(|relative| {
                        Self::parse_btrfs_relative_path(
                            relative,
                            base_mount_metadata,
                            opt_subvol,
                            map_of_datasets,
                        )
                    })
                    .collect()
            }) {
            Some(vec) => vec,
            None => {
                eprintln!("WARN: No snaps found for mount: {:?}", base_mount);
                Vec::new()
            }
        }
    }

    fn parse_btrfs_relative_path(
        relative: &Path,
        base_mount_metadata: &DatasetMetadata,
        opt_subvol: &Option<PathBuf>,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
    ) -> Option<PathBuf> {
        let mut path_iter = relative.components();

        let opt_dataset = path_iter.next();

        let the_rest = path_iter;

        match opt_dataset
            .and_then(|dataset| {
                map_of_datasets.iter().find_map(|(mount, metadata)| {
                    if metadata.source != base_mount_metadata.source {
                        return None;
                    }

                    opt_subvol.as_ref().and_then(|subvol| {
                        let needle = dataset.as_os_str().to_string_lossy();
                        let haystack = subvol.to_string_lossy();

                        if haystack.ends_with(needle.as_ref()) {
                            Some(mount)
                        } else {
                            None
                        }
                    })
                })
            })
            .map(|mount| mount.join(the_rest))
        {
            Some(snap_mount) if snap_mount.exists() => {
                return Some(snap_mount);
            }
            _ => {
                let btrfs_root = map_of_datasets
                    .iter()
                    .find(|(_mount, metadata)| match &metadata.fs_type {
                        FilesystemType::Btrfs(Some(subvol)) => {
                            metadata.source == base_mount_metadata.source
                                && subvol == BTRFS_ROOT_SUBVOL.as_path()
                        }
                        _ => false,
                    })
                    .map(|(mount, _metadata)| mount.to_owned())
                    .unwrap_or_else(|| PathBuf::from(ROOT_DIRECTORY));

                let snap_mount = btrfs_root.to_path_buf().join(relative);

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
    ) -> Vec<PathBuf> {
        fn inner(
            mount_point_path: &Path,
            dataset_metadata: &DatasetMetadata,
        ) -> Result<Vec<PathBuf>, std::io::Error> {
            let snaps = match dataset_metadata.fs_type {
                FilesystemType::Btrfs(_) => {
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
                        .filter(|mount_info| {
                            mount_info.options.iter().any(|opt| opt.contains("cp="))
                        })
                        .map(|mount_info| PathBuf::from(mount_info.dest))
                        .collect()
                }
            };

            Ok(snaps)
        }

        match inner(mount_point_path, dataset_metadata) {
            Ok(res) => res,
            Err(_err) => Vec::new(),
        }
    }
}
