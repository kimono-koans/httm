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
use once_cell::sync::OnceCell;
use proc_mounts::MountIter;
use rayon::prelude::*;
use which::which;

use crate::library::results::{HttmError, HttmResult};
use crate::parse::aliases::FilesystemType;
use crate::parse::mounts::{DatasetMetadata, MountType};
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX, ROOT_DIRECTORY, ZFS_SNAPSHOT_DIRECTORY,
};

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
        let root_mount_path: OnceCell<Option<&PathBuf>> = OnceCell::new();

        let map_of_snaps: HashMap<PathBuf, Vec<PathBuf>> = map_of_datasets
            .par_iter()
            .flat_map(|(mount, dataset_info)| {
                let snap_mounts: HttmResult<Vec<PathBuf>> = match dataset_info.fs_type {
                    FilesystemType::Zfs | FilesystemType::Nilfs2 => {
                        Self::from_defined_mounts(mount, dataset_info)
                    }
                    FilesystemType::Btrfs => {
                        root_mount_path.get_or_init(|| Self::btrfs_root(map_of_datasets));

                        match root_mount_path
                            .get()
                            .expect("get_or_init() should have set this value")
                        {
                            Some(root_mount_path) => match dataset_info.mount_type {
                                MountType::Local => Self::from_btrfs_cmd(mount, root_mount_path),
                                MountType::Network => {
                                    Self::from_defined_mounts(mount, dataset_info)
                                }
                            },
                            None => Self::from_btrfs_cmd(mount, mount),
                        }
                    }
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

    fn btrfs_root(map_of_datasets: &HashMap<PathBuf, DatasetMetadata>) -> Option<&PathBuf> {
        let root_dir = Path::new(ROOT_DIRECTORY);

        map_of_datasets
            .iter()
            .filter(|(_mount, dataset_info)| dataset_info.fs_type == FilesystemType::Btrfs)
            .find(|(_mount, dataset_info)| dataset_info.source == root_dir)
            .map(|(mount, _dataset_info)| mount)
    }

    // build paths to all snap mounts
    fn from_btrfs_cmd(mount_point_path: &Path, root_mount_path: &Path) -> HttmResult<Vec<PathBuf>> {
        let btrfs_command = which("btrfs").map_err(|_err| {
            HttmError::new(
                "'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
            )
        })?;

        let exec_command = btrfs_command;
        let arg_path = mount_point_path.to_string_lossy();
        let args = vec!["subvolume", "list", "-a", "-s", &arg_path];

        // must exec for each mount, probably a better way by calling into a lib
        let command_output =
            std::str::from_utf8(&ExecProcess::new(exec_command).args(&args).output()?.stdout)?
                .to_owned();

        let snaps = command_output
            .par_lines()
            .filter_map(|line| line.split_once("path "))
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
                    .filter(|mount_info| mount_info.source.as_path() == source_path)
                    .filter(|mount_info| mount_info.options.iter().any(|opt| opt.contains("cp=")))
                    .map(|mount_info| mount_info.dest)
                    .collect()
            }
        };

        Ok(snaps)
    }
}
