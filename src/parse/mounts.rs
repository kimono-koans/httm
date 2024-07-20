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
use crate::library::utility::{find_common_path, fs_type_from_hidden_dir};
use crate::parse::snaps::MapOfSnaps;
use crate::{
    NILFS2_SNAPSHOT_ID_KEY, ROOT_DIRECTORY, TM_DIR_LOCAL, TM_DIR_REMOTE, ZFS_HIDDEN_DIRECTORY,
};
use hashbrown::{HashMap, HashSet};
use once_cell::sync::Lazy;
use proc_mounts::MountIter;
use rayon::iter::Either;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command as ExecProcess;
use which::which;

pub const ZFS_FSTYPE: &str = "zfs";
pub const NILFS2_FSTYPE: &str = "nilfs2";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";
pub const FUSE_FSTYPE_LINUX: &str = "fuse";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilesystemType {
    Zfs,
    Btrfs(Option<PathBuf>),
    Nilfs2,
    Apfs,
    Restic(Option<Vec<PathBuf>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetMetadata {
    pub source: PathBuf,
    pub fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDirs {
    inner: HashSet<PathBuf>,
}

impl Deref for FilterDirs {
    type Target = HashSet<PathBuf>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub trait MaxLen {
    fn max_len(&self) -> usize;
}

impl MaxLen for FilterDirs {
    fn max_len(&self) -> usize {
        self.inner
            .iter()
            .map(|dir| dir.components().count())
            .max()
            .unwrap_or(usize::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfDatasets {
    inner: HashMap<PathBuf, DatasetMetadata>,
}

impl Deref for MapOfDatasets {
    type Target = HashMap<PathBuf, DatasetMetadata>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MaxLen for MapOfDatasets {
    fn max_len(&self) -> usize {
        self.inner
            .keys()
            .map(|mount| mount.components().count())
            .max()
            .unwrap_or(usize::MAX)
    }
}

pub static PROC_MOUNTS: Lazy<PathBuf> = Lazy::new(|| PathBuf::from("/proc/mounts"));
pub static BTRFS_ROOT_SUBVOL: Lazy<PathBuf> = Lazy::new(|| PathBuf::from("<FS_TREE>"));
pub static ROOT_PATH: Lazy<PathBuf> = Lazy::new(|| PathBuf::from(ROOT_DIRECTORY));
static ETC_MNTTAB: Lazy<PathBuf> = Lazy::new(|| PathBuf::from("/etc/mnttab"));
static RESTIC_SOURCE_PATH: Lazy<PathBuf> = Lazy::new(|| PathBuf::from("restic"));
static TM_DIR_REMOTE_PATH: Lazy<PathBuf> = Lazy::new(|| PathBuf::from(TM_DIR_REMOTE));
static TM_DIR_LOCAL_PATH: Lazy<PathBuf> = Lazy::new(|| PathBuf::from(TM_DIR_LOCAL));

pub struct BaseFilesystemInfo {
    pub map_of_datasets: MapOfDatasets,
    pub map_of_snaps: MapOfSnaps,
    pub filter_dirs: FilterDirs,
}

impl BaseFilesystemInfo {
    // divide by the type of system we are on
    // Linux allows us the read proc mounts
    pub fn new(opt_debug: bool, opt_alt_backup: Option<&String>) -> HttmResult<Self> {
        let (mut raw_datasets, filter_dirs_set) = if PROC_MOUNTS.exists() {
            Self::from_file(&PROC_MOUNTS)?
        } else if ETC_MNTTAB.exists() {
            Self::from_file(&ETC_MNTTAB)?
        } else {
            Self::from_mount_cmd()?
        };

        match opt_alt_backup.map(|res| res.as_str()) {
            Some("timemachine") => Self::from_tm_dir(&mut raw_datasets)?,
            Some("restic") => Self::from_restic_dir(&mut raw_datasets)?,
            _ => {}
        }

        let map_of_snaps = MapOfSnaps::new(&raw_datasets, opt_debug)?;

        let map_of_datasets = {
            MapOfDatasets {
                inner: raw_datasets,
            }
        };

        let filter_dirs = {
            FilterDirs {
                inner: filter_dirs_set,
            }
        };

        Ok(BaseFilesystemInfo {
            map_of_datasets,
            map_of_snaps,
            filter_dirs,
        })
    }

    // parsing from proc mounts is both faster and necessary for certain btrfs features
    // for instance, allows us to read subvolumes mounts, like "/@" or "/@home"
    fn from_file(path: &Path) -> HttmResult<(HashMap<PathBuf, DatasetMetadata>, HashSet<PathBuf>)> {
        let mount_iter = MountIter::new_from_file(path)?;

        let (map_of_datasets, filter_dirs): (HashMap<PathBuf, DatasetMetadata>, HashSet<PathBuf>) =
            mount_iter
                .par_bridge()
                .flatten()
                .filter(|mount_info| {
                    !mount_info
                        .dest
                        .to_string_lossy()
                        .contains(ZFS_HIDDEN_DIRECTORY)
                })
                .filter(|mount_info| {
                    !mount_info
                        .options
                        .iter()
                        .any(|opt| opt.contains(NILFS2_SNAPSHOT_ID_KEY))
                })
                .map(|mount_info| {
                    let dest_path = PathBuf::from(&mount_info.dest);
                    (mount_info, dest_path)
                })
                .partition_map(|(mount_info, dest_path)| match mount_info.fstype.as_str() {
                    ZFS_FSTYPE => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Zfs,
                        },
                    )),
                    SMB_FSTYPE | AFP_FSTYPE | NFS_FSTYPE => {
                        match fs_type_from_hidden_dir(&dest_path) {
                            Some(FilesystemType::Zfs) => Either::Left((
                                dest_path,
                                DatasetMetadata {
                                    source: PathBuf::from(mount_info.source),
                                    fs_type: FilesystemType::Zfs,
                                },
                            )),
                            Some(FilesystemType::Btrfs(None)) => Either::Left((
                                dest_path,
                                DatasetMetadata {
                                    source: PathBuf::from(mount_info.source),
                                    fs_type: FilesystemType::Btrfs(None),
                                },
                            )),
                            _ => Either::Right(dest_path),
                        }
                    }
                    BTRFS_FSTYPE => {
                        let keyed_options: BTreeMap<&str, &str> = mount_info
                            .options
                            .iter()
                            .filter(|line| line.contains('='))
                            .filter_map(|line| line.split_once('='))
                            .collect();

                        let opt_subvol = keyed_options.get("subvol").map(|subvol| {
                            match keyed_options.get("subvolid") {
                                Some(id) if *id == "5" => BTRFS_ROOT_SUBVOL.clone(),
                                _ => PathBuf::from(subvol),
                            }
                        });

                        Either::Left((
                            dest_path,
                            DatasetMetadata {
                                source: mount_info.source,
                                fs_type: FilesystemType::Btrfs(opt_subvol),
                            },
                        ))
                    }
                    NILFS2_FSTYPE => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Nilfs2,
                        },
                    )),
                    FUSE_FSTYPE_LINUX if mount_info.source == *RESTIC_SOURCE_PATH => {
                        Either::Left((
                            dest_path,
                            DatasetMetadata {
                                source: mount_info.source,
                                fs_type: FilesystemType::Restic(None),
                            },
                        ))
                    }
                    _ => Either::Right(dest_path),
                });

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, filter_dirs))
        }
    }

    pub fn from_restic_dir(
        map_of_datasets: &mut HashMap<PathBuf, DatasetMetadata>,
    ) -> HttmResult<()> {
        map_of_datasets.retain(|_k, v| matches!(v.fs_type, FilesystemType::Restic(_)));

        if map_of_datasets.is_empty() {
            return Err(HttmError::new(
                "ERROR: No supported Restic datasets were found on the system.",
            )
            .into());
        }

        let mut new = HashMap::new();

        let repos = map_of_datasets.keys().cloned().collect();

        let metadata = DatasetMetadata {
            source: PathBuf::from("restic"),
            fs_type: FilesystemType::Restic(Some(repos)),
        };

        new.insert_unique_unchecked(ROOT_PATH.clone(), metadata);

        *map_of_datasets = new;

        return Ok(());
    }

    pub fn from_tm_dir(map_of_datasets: &mut HashMap<PathBuf, DatasetMetadata>) -> HttmResult<()> {
        if !cfg!(target_os = "macos") {
            return Err(HttmError::new(
                "ERROR: Time Machine is only supported on Mac OS.  This appears to be an unsupported OS."
            )
            .into());
        }

        if !TM_DIR_REMOTE_PATH.exists() && !TM_DIR_LOCAL_PATH.exists() {
            return Err(HttmError::new(
                "ERROR: Neither a local nor a remote Time Machine path seems to exist for this system."
            )
            .into());
        }

        let mut new = HashMap::new();

        let metadata = DatasetMetadata {
            source: PathBuf::from("timemachine"),
            fs_type: FilesystemType::Apfs,
        };

        new.insert_unique_unchecked(ROOT_PATH.clone(), metadata);

        *map_of_datasets = new;

        Ok(())
    }

    // old fashioned parsing for non-Linux systems, nearly as fast, works everywhere with a mount command
    // both methods are much faster than using zfs command
    fn from_mount_cmd() -> HttmResult<(HashMap<PathBuf, DatasetMetadata>, HashSet<PathBuf>)> {
        // do we have the necessary commands for search if user has not defined a snap point?
        // if so run the mount search, if not print some errors
        let mount_command = which("mount").map_err(|_err| {
            HttmError::new(
                "'mount' command not be found. Make sure the command 'mount' is in your path.",
            )
        })?;

        let command_output = &ExecProcess::new(mount_command).output()?;

        let stderr_string = std::str::from_utf8(&command_output.stderr)?;

        if !stderr_string.is_empty() {
            return Err(HttmError::new(stderr_string).into());
        }

        let stdout_string = std::str::from_utf8(&command_output.stdout)?;

        // parse "mount" for filesystems and mountpoints
        let (mut map_of_datasets, filter_dirs): (
            HashMap<PathBuf, DatasetMetadata>,
            HashSet<PathBuf>,
        ) = stdout_string
            .par_lines()
            // but exclude snapshot mounts.  we want the raw filesystem names.
            .filter(|line| !line.contains(ZFS_HIDDEN_DIRECTORY))
            .filter(|line| !line.contains(TM_DIR_REMOTE))
            .filter(|line| !line.contains(TM_DIR_LOCAL))
            // mount cmd includes and " on " between src and rest
            .filter_map(|line| line.split_once(" on "))
            // where to split, to just have the src and dest of mounts
            .filter_map(|(filesystem, rest)| {
                // GNU Linux mount output
                if rest.contains("type") {
                    let opt_mount = rest.split_once(" type");
                    opt_mount.map(|mount| (filesystem, mount.0))
                // Busybox and BSD mount output
                } else if rest.contains(" (") {
                    let opt_mount = rest.split_once(" (");
                    opt_mount.map(|mount| (filesystem, mount.0))
                } else {
                    None
                }
            })
            .map(|(filesystem, mount)| (PathBuf::from(filesystem), PathBuf::from(mount)))
            // sanity check: does the filesystem exist and have a ZFS hidden dir? if not, filter it out
            // and flip around, mount should key of key/value
            .partition_map(|(source, mount)| match fs_type_from_hidden_dir(&mount) {
                Some(FilesystemType::Zfs) => Either::Left((
                    mount,
                    DatasetMetadata {
                        source,
                        fs_type: FilesystemType::Zfs,
                    },
                )),
                Some(FilesystemType::Btrfs(_)) => Either::Left((
                    mount,
                    DatasetMetadata {
                        source,
                        fs_type: FilesystemType::Btrfs(None),
                    },
                )),
                _ if source == *RESTIC_SOURCE_PATH => Either::Left((
                    mount,
                    DatasetMetadata {
                        source,
                        fs_type: FilesystemType::Restic(None),
                    },
                )),
                _ => Either::Right(mount),
            });

        if TM_DIR_REMOTE_PATH.exists() || TM_DIR_LOCAL_PATH.exists() {
            match map_of_datasets.get(ROOT_PATH.as_path()) {
                Some(_root) => {}
                None => {
                    let metadata = DatasetMetadata {
                        source: PathBuf::from("timemachine"),
                        fs_type: FilesystemType::Apfs,
                    };

                    map_of_datasets.insert_unique_unchecked(ROOT_PATH.to_path_buf(), metadata);
                }
            }
        }

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, filter_dirs))
        }
    }

    // if we have some btrfs mounts, we check to see if there is a snap directory in common
    // so we can hide that common path from searches later
    pub fn common_snap_dir(&self) -> Option<PathBuf> {
        let map_of_datasets: &MapOfDatasets = &self.map_of_datasets;
        let map_of_snaps: &MapOfSnaps = &self.map_of_snaps;

        if map_of_datasets
            .par_iter()
            .any(|(_mount, dataset_info)| matches!(dataset_info.fs_type, FilesystemType::Btrfs(_)))
        {
            let vec_snaps: Vec<&PathBuf> = map_of_datasets
                .par_iter()
                .filter(|(_mount, dataset_info)| {
                    if matches!(dataset_info.fs_type, FilesystemType::Btrfs(_)) {
                        return true;
                    }

                    false
                })
                .filter_map(|(mount, _dataset_info)| map_of_snaps.get(mount))
                .flatten()
                .collect();

            return find_common_path(vec_snaps);
        }

        None
    }
}
