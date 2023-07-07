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

use std::{fs::read_dir, ops::Deref, path::Path, path::PathBuf, process::Command as ExecProcess};

use hashbrown::HashMap;
use proc_mounts::MountIter;
use rayon::prelude::*;
use which::which;

use crate::library::results::{HttmError, HttmResult};
use crate::parse::aliases::FilesystemType;
use crate::parse::mounts::{DatasetMetadata, MountType};
use crate::{BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX, ZFS_SNAPSHOT_DIRECTORY};

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
            .flat_map(|(mount, dataset_info)| {
                let snap_mounts: HttmResult<Vec<PathBuf>> = match dataset_info.fs_type {
                    FilesystemType::Zfs | FilesystemType::Nilfs2 => {
                        Self::from_defined_mounts(mount, dataset_info)
                    }
                    FilesystemType::Btrfs => match dataset_info.mount_type {
                        MountType::Local => Self::from_btrfs_cmd(mount),
                        MountType::Network => Self::from_defined_mounts(mount, dataset_info),
                    },
                };

                snap_mounts.map(|snap_mounts| (mount.clone(), snap_mounts))
            })
            .collect();

        if map_of_snaps.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(map_of_snaps.into())
        }
    }

    // build paths to all snap mounts
    fn from_btrfs_cmd(mount: &Path) -> HttmResult<Vec<PathBuf>> {
        let btrfs_command = which("btrfs").map_err(|_err| {
            HttmError::new(
                "'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
            )
        })?;

        let exec_command = btrfs_command;
        let arg_path = mount.to_string_lossy();
        let args = vec!["subvolume", "show", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        let snaps = command_output
            .split_once("Snapshot(s):\n")
            .map(|(pre, snap_paths)| {
                snap_paths
                    .lines()
                    .map(|line| line.trim())
                    .map(|relative| {
                        if pre.contains("<FS_TREE>") {
                            // "<FS_TREE>/" should be the root path
                            mount.join(relative)
                        } else {
                            // btrfs sub list -a -s output includes the sub name (eg @home)
                            // when that sub could be mounted anywhere, so we remove here
                            let snap_path_parsed: PathBuf =
                                Path::new(relative).components().skip(1).collect();

                            mount.join(snap_path_parsed)
                        }
                    })
                    .filter(|snap| snap.exists())
                    .collect()
            })
            .ok_or_else(|| {
                let msg = format!("No snaps found for mount: {:?}", mount);
                HttmError::new(&msg)
            })?;

        Ok(snaps)
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
            FilesystemType::Nilfs2 => {
                let source_path = Path::new(&dataset_metadata.source);

                MountIter::new()?
                    .flatten()
                    .par_bridge()
                    .filter(|mount_info| mount_info.source == source_path)
                    .filter(|mount_info| mount_info.options.iter().any(|opt| opt.contains("cp=")))
                    .map(|mount_info| mount_info.dest)
                    .collect()
            }
        };

        Ok(snaps)
    }
}
