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
use crate::library::utility::{get_btrfs_command, user_has_effective_root};
use crate::parse::mounts::BTRFS_ROOT_SUBVOL;
use crate::parse::mounts::PROC_MOUNTS;
use crate::parse::mounts::{DatasetMetadata, FilesystemType};
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY, BTRFS_SNAPPER_SUFFIX, RESTIC_SNAPSHOT_DIRECTORY,
    ROOT_DIRECTORY, TM_DIR_LOCAL, TM_DIR_REMOTE, ZFS_SNAPSHOT_DIRECTORY,
};
use hashbrown::HashMap;
use proc_mounts::MountIter;
use rayon::prelude::*;
use std::fs::read_dir;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::sync::Once;

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
    pub fn new(
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
        opt_debug: bool,
    ) -> HttmResult<Self> {
        let map_of_snaps: HashMap<PathBuf, Vec<PathBuf>> = map_of_datasets
            .par_iter()
            .map(|(mount, dataset_info)| {      
                let snap_mounts: Vec<PathBuf> = match &dataset_info.fs_type {
                    FilesystemType::Zfs | FilesystemType::Nilfs2 | FilesystemType::Apfs | FilesystemType::Restic(_) | FilesystemType::Btrfs(None) => {
                        Self::from_defined_mounts(mount, dataset_info)
                    }
                    // btrfs Some mounts are potential local mount
                    FilesystemType::Btrfs(Some(additional_data)) => {
                        let map = Self::from_btrfs_cmd(
                            mount,
                            dataset_info,
                            &additional_data.base_subvol,
                            map_of_datasets,
                            opt_debug,
                        );

                        if map.is_empty() {
                            static NOTICE_FALLBACK: Once = Once::new();

                            NOTICE_FALLBACK.call_once(|| {
                                eprintln!(
                                    "NOTICE: Falling back to detection of btrfs snapshot mounts perhaps defined by Snapper re: mount: {:?}", mount
                                );
                            });

                            Self::from_defined_mounts(mount, dataset_info)
                        } else {
                            additional_data.snap_names.get_or_init(|| {
                                map.clone()
                            });

                            map.into_keys().collect()
                        }
                    }
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
    pub fn from_btrfs_cmd(
        base_mount: &Path,
        base_mount_metadata: &DatasetMetadata,
        base_subvol: &Path,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
        opt_debug: bool,
    ) -> HashMap<PathBuf, PathBuf> {
        const BTRFS_COMMAND_REQUIRES_ROOT: &str =
            "btrfs mounts detected.  User must have super user permissions to determine the location of btrfs snapshots";

        if let Err(_err) = user_has_effective_root(&BTRFS_COMMAND_REQUIRES_ROOT) {
            static USER_HAS_ROOT_WARNING: Once = Once::new();

            USER_HAS_ROOT_WARNING.call_once(|| {
                eprintln!("WARN: {}", BTRFS_COMMAND_REQUIRES_ROOT);
            });
            return HashMap::new();
        }

        let Ok(btrfs_command) = get_btrfs_command() else {
            static BTRFS_COMMAND_AVAILABLE_WARNING: Once = Once::new();

            BTRFS_COMMAND_AVAILABLE_WARNING.call_once(|| {
                eprintln!(
                    "WARN: 'btrfs' command not found. Make sure the command 'btrfs' is in your path.",
                );
            });

            return HashMap::new();
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
                    .map(|string| string.trim().to_owned())
                    .ok()
            })
        else {
            static COULD_NOT_OBTAIN_BTRFS_COMMAND_OUTPUT: Once = Once::new();

            COULD_NOT_OBTAIN_BTRFS_COMMAND_OUTPUT.call_once(|| {
                eprintln!("WARN: Could not obtain btrfs command output.",);
            });
            return HashMap::new();
        };

        match command_output
            .split_once("Snapshot(s):\n")
            .map(|(_first, last)| match last.rsplit_once("Quota group:") {
                Some((snap_paths, _remainder)) => snap_paths,
                None => last,
            })
            .map(|snap_paths| {
                snap_paths
                    .par_lines()
                    .map(|line| line.trim())
                    .map(|line| Path::new(line))
                    .filter(|line| !line.as_os_str().is_empty())
                    .filter_map(|snap_name| {
                        let opt_snap_location = Self::parse_btrfs_relative_path(
                            base_mount,
                            &base_mount_metadata.source,
                            base_subvol,
                            snap_name,
                            map_of_datasets,
                            opt_debug,
                        );

                        opt_snap_location
                            .map(|snap_location| (snap_location, snap_name.to_path_buf()))
                    })
                    .collect()
            }) {
            Some(map) => map,
            None => {
                //eprintln!("WARN: No snaps found for mount: {:?}", base_mount);
                HashMap::new()
            }
        }
    }

    fn parse_btrfs_relative_path(
        base_mount: &Path,
        base_mount_source: &Path,
        base_subvol: &Path,
        snap_relative: &Path,
        map_of_datasets: &HashMap<PathBuf, DatasetMetadata>,
        opt_debug: bool,
    ) -> Option<PathBuf> {
        let mut path_iter = snap_relative.components();

        let opt_first_snap_component = path_iter.next();

        let the_rest = path_iter;

        if opt_debug {
            eprintln!(
                "DEBUG: Base mount: {:?}, Base subvol: {:?}, Snap Relative Path: {:?}",
                base_mount, base_subvol, snap_relative
            );
        }

        match opt_first_snap_component
            .and_then(|first_snap_component| {
                // btrfs subvols usually look like /@subvol in mounts info, but are listed elsewhere
                // such as the first snap component, as @subvol, so here we remove the leading "/"
                let potential_dataset = first_snap_component.as_os_str().to_string_lossy();
                let base_subvol_name = base_subvol.to_string_lossy();

                // short circuit -- if subvol is same as dataset return base mount
                if potential_dataset == base_subvol_name.trim_start_matches("/") {
                    return Some(base_mount);
                }

                map_of_datasets.iter().find_map(|(mount, metadata)| {
                    // if the datasets do not match then can't be the same btrfs subvol
                    if metadata.source != base_mount_source {
                        return None;
                    }

                    match &metadata.fs_type {
                        FilesystemType::Btrfs(Some(additional_data)) => {
                            let subvol_name = additional_data.base_subvol.to_string_lossy();

                            if potential_dataset == subvol_name.trim_start_matches("/") {
                                Some(mount.as_path())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                })
            })
            .map(|mount| {
                let joined = mount.join(the_rest);

                if opt_debug {
                    eprintln!("DEBUG: Joined path: {:?}", joined);
                }

                joined
            }) {
            // here we check if the path actually exists because of course this is inexact!
            Some(snap_mount) => {
                if snap_mount.exists() {
                    Some(snap_mount)
                } else {
                    eprintln!(
                        "WARN: Snapshot mount requested does not exist or perhaps is not mounted: {:?}",
                        snap_relative
                    );
                    None
                }
            }
            None => {
                // btrfs root is different for each device, here, we check to see they have the same device
                // and when we parse mounts we check to see that they have a subvolid of "5", then we replace
                // whatever subvol name with a special id: <FS_TREE>
                let btrfs_root = map_of_datasets
                    .iter()
                    .find(|(_mount, metadata)| match &metadata.fs_type {
                        FilesystemType::Btrfs(Some(additional_data)) => {
                            metadata.source == base_mount_source
                                && additional_data.base_subvol == BTRFS_ROOT_SUBVOL.as_path()
                        }
                        _ => false,
                    })
                    .map(|(mount, _metadata)| mount.to_owned())
                    .unwrap_or_else(|| PathBuf::from(ROOT_DIRECTORY));

                let snap_mount = btrfs_root.join(snap_relative);

                if opt_debug {
                    eprintln!(
                        "DEBUG: Btrfs top level {:?}, Snap Mount: {:?}",
                        btrfs_root, snap_mount
                    );
                }

                // here we check if the path actually exists because of course this is inexact!
                if snap_mount.exists() {
                    Some(snap_mount)
                } else {
                    eprintln!(
                        "WARN: Snapshot mount requested does not exist or perhaps is not mounted: {:?}",
                        snap_relative
                    );
                    None
                }
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
        ) -> HttmResult<Vec<PathBuf>> {
            let snaps = match &dataset_metadata.fs_type {
                FilesystemType::Btrfs(_) => {
                    read_dir(mount_point_path.join(BTRFS_SNAPPER_HIDDEN_DIRECTORY))?
                        .flatten()
                        .par_bridge()
                        .map(|entry| entry.path().join(BTRFS_SNAPPER_SUFFIX))
                        .collect()
                }
                FilesystemType::Restic(None) => {
                    // base is latest, parent is the snap path
                    let repos = mount_point_path.parent();

                    repos
                        .iter()
                        .flat_map(|repo| read_dir(repo))
                        .flatten()
                        .flatten()
                        .map(|dir_entry| dir_entry.path())
                        .filter(|path| !path.ends_with("latest"))
                        .collect()
                }
                FilesystemType::Restic(Some(repos)) => repos
                    .par_iter()
                    .flat_map(|repo| read_dir(repo.join(RESTIC_SNAPSHOT_DIRECTORY)))
                    .flatten_iter()
                    .flatten()
                    .map(|dir_entry| dir_entry.path())
                    .filter(|path| !path.ends_with("latest"))
                    .collect(),
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
