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
// that was distributed wth this source code.

use super::aliases::MapOfAliases;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{find_common_path, get_mount_command};
use crate::parse::snaps::MapOfSnaps;
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY,
    GLOBAL_CONFIG,
    NILFS2_SNAPSHOT_ID_KEY,
    RESTIC_LATEST_SNAPSHOT_DIRECTORY,
    TM_DIR_LOCAL,
    TM_DIR_REMOTE,
    ZFS_HIDDEN_DIRECTORY,
    ZFS_SNAPSHOT_DIRECTORY,
};
use proc_mounts::MountIter;
use rayon::iter::Either;
use rayon::prelude::*;
use realpath_ext::{realpath, RealpathFlags};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::sync::{Arc, LazyLock, OnceLock};

pub const ZFS_FSTYPE: &str = "zfs";
pub const NILFS2_FSTYPE: &str = "nilfs2";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";
pub const RESTIC_FSTYPE: &str = "restic";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LinkType {
    Local,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtrfsAdditionalData {
    pub base_subvol: PathBuf,
    pub snap_names: OnceLock<BTreeMap<PathBuf, PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResticAdditionalData {
    pub repos: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemType {
    Zfs,
    Btrfs(Option<Box<BtrfsAdditionalData>>),
    Nilfs2,
    Apfs,
    Restic(Option<Box<ResticAdditionalData>>),
}

impl FilesystemType {
    pub fn new(dataset_mount: &Path) -> Option<FilesystemType> {
        // set fstype, known by whether there is a ZFS hidden snapshot dir in the root dir
        if dataset_mount
            .join(ZFS_SNAPSHOT_DIRECTORY)
            .symlink_metadata()
            .is_ok()
        {
            Some(FilesystemType::Zfs)
        } else if dataset_mount
            .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
            .symlink_metadata()
            .is_ok()
        {
            Some(FilesystemType::Btrfs(None))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetMetadata {
    pub source: PathBuf,
    pub fs_type: FilesystemType,
    pub link_type: LinkType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDirs {
    inner: BTreeSet<Arc<Path>>,
}

impl Deref for FilterDirs {
    type Target = BTreeSet<Arc<Path>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl FilterDirs {
    pub fn is_filter_dir(&self, path: &Path) -> bool {
        self.iter().any(|filter_dir| path == filter_dir.as_ref())
    }
}

pub trait IsFilterDir {
    fn is_filter_dir(&self) -> bool;
}

impl<T: AsRef<Path>> IsFilterDir for T
where
    T: AsRef<Path>,
{
    fn is_filter_dir(self: &T) -> bool {
        GLOBAL_CONFIG
            .dataset_collection
            .filter_dirs
            .is_filter_dir(self.as_ref())
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
    inner: BTreeMap<Arc<Path>, DatasetMetadata>,
}

impl Deref for MapOfDatasets {
    type Target = BTreeMap<Arc<Path>, DatasetMetadata>;

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

pub static PROC_MOUNTS: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/proc/mounts"));
pub static BTRFS_ROOT_SUBVOL: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("<FS_TREE>"));
pub static ROOT_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/"));
static ETC_MNTTAB: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/etc/mnttab"));
static TM_DIR_REMOTE_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from(TM_DIR_REMOTE));
static TM_DIR_LOCAL_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from(TM_DIR_LOCAL));

pub struct BaseFilesystemInfo {
    pub map_of_datasets: MapOfDatasets,
    pub map_of_snaps: MapOfSnaps,
    pub filter_dirs: FilterDirs,
}

impl BaseFilesystemInfo {
    // divide by the type of system we are on
    // Linux allows us the read proc mounts
    pub fn new(
        opt_debug: bool,
        opt_alt_store: &mut Option<FilesystemType>,
        opt_map_of_aliases: &Option<MapOfAliases>,
    ) -> HttmResult<Self> {
        let (mut raw_datasets, filter_dirs_set) = if PROC_MOUNTS.exists() {
            Self::from_file(&PROC_MOUNTS, opt_alt_store)?
        } else if ETC_MNTTAB.exists() {
            Self::from_file(&ETC_MNTTAB, opt_alt_store)?
        } else {
            Self::from_mount_cmd(opt_alt_store)?
        };

        // prep any blob repos
        if let Some(repo_type) = opt_alt_store {
            Self::from_blob_repo(&mut raw_datasets, repo_type)?;
        }

        if raw_datasets.is_empty() {
            // auto enable time machine alt store on mac when no datasets available, no working aliases, and paths exist
            if cfg!(target_os = "macos")
                && opt_map_of_aliases.is_none()
                && TM_DIR_REMOTE_PATH.exists()
                && TM_DIR_LOCAL_PATH.exists()
            {
                opt_alt_store.replace(FilesystemType::Apfs);
                Self::from_blob_repo(&mut raw_datasets, &FilesystemType::Apfs)?;
            } else {
                return Err(HttmError::new(
                    "httm could not find any valid datasets on the system.",
                )
                .into());
            }
        }

        let map_of_snaps = MapOfSnaps::new(&mut raw_datasets, opt_debug)?;

        if map_of_snaps.iter().any(|(_mount, snaps)| snaps.is_empty()) {
            if opt_debug {
                eprintln!("WARN: httm relies on the user (and/or the filesystem's auto-mounter) to mount snapshots.  Make certain any snapshots the user may want to view are mounted, or are able to be mounted, and/or the user has the correct permissions to view.");
            }

            if map_of_snaps.values().count() == 0 {
                return Err(HttmError::new(
                    "httm could not find any valid snapshots on the system.  Quitting.",
                )
                .into());
            }

            if opt_debug {
                map_of_snaps.iter().for_each(|(mount, snaps)| {
                    if snaps.is_empty() {
                        eprintln!(
                            "WARN: Mount {:?} appears to have no snapshots available.",
                            mount
                        )
                    }
                })
            }
        }

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
    fn from_file(
        path: &Path,
        opt_alt_store: &Option<FilesystemType>,
    ) -> HttmResult<(BTreeMap<Arc<Path>, DatasetMetadata>, BTreeSet<Arc<Path>>)> {
        let mount_iter = MountIter::new_from_file(path)?;

        let (map_of_datasets, filter_dirs): (
            BTreeMap<Arc<Path>, DatasetMetadata>,
            BTreeSet<Arc<Path>>,
        ) = mount_iter
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
                    Arc::from(dest_path),
                    DatasetMetadata {
                        source: PathBuf::from(mount_info.source),
                        fs_type: FilesystemType::Zfs,
                        link_type: LinkType::Local,
                    },
                )),
                SMB_FSTYPE | AFP_FSTYPE | NFS_FSTYPE => match FilesystemType::new(&dest_path) {
                    Some(FilesystemType::Zfs) => Either::Left((
                        Arc::from(dest_path),
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Zfs,
                            link_type: LinkType::Network,
                        },
                    )),
                    Some(FilesystemType::Btrfs(None)) => Either::Left((
                        Arc::from(dest_path),
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Btrfs(None),
                            link_type: LinkType::Network,
                        },
                    )),
                    _ => Either::Right(Arc::from(dest_path)),
                },
                BTRFS_FSTYPE => {
                    let keyed_options: BTreeMap<&str, &str> = mount_info
                        .options
                        .iter()
                        .filter(|line| line.contains('='))
                        .filter_map(|line| line.split_once('='))
                        .collect();

                    let opt_additional_data = keyed_options
                        .get("subvol")
                        .map(|subvol| match keyed_options.get("subvolid") {
                            Some(id) if *id == "5" => BTRFS_ROOT_SUBVOL.clone(),
                            _ => PathBuf::from(subvol),
                        })
                        .map(|base_subvol| {
                            Box::new(BtrfsAdditionalData {
                                base_subvol,
                                snap_names: OnceLock::new(),
                            })
                        });

                    Either::Left((
                        Arc::from(dest_path),
                        DatasetMetadata {
                            source: mount_info.source,
                            fs_type: FilesystemType::Btrfs(opt_additional_data),
                            link_type: LinkType::Local,
                        },
                    ))
                }
                NILFS2_FSTYPE => Either::Left((
                    Arc::from(dest_path),
                    DatasetMetadata {
                        source: PathBuf::from(mount_info.source),
                        fs_type: FilesystemType::Nilfs2,
                        link_type: LinkType::Local,
                    },
                )),
                _ if mount_info.source.to_string_lossy().contains(RESTIC_FSTYPE) => {
                    let base_path = if let Some(FilesystemType::Restic(_)) = opt_alt_store {
                        dest_path
                    } else {
                        dest_path.join(RESTIC_LATEST_SNAPSHOT_DIRECTORY)
                    };

                    let canonical_path: PathBuf =
                        realpath(&base_path, RealpathFlags::ALLOW_MISSING)
                            .unwrap_or_else(|_| base_path.to_path_buf());

                    Either::Left((
                        Arc::from(canonical_path),
                        DatasetMetadata {
                            source: mount_info.source,
                            fs_type: FilesystemType::Restic(None),
                            link_type: LinkType::Local,
                        },
                    ))
                }
                _ => Either::Right(Arc::from(dest_path)),
            });

        Ok((map_of_datasets, filter_dirs))
    }

    // old fashioned parsing for non-Linux systems, nearly as fast, works everywhere with a mount command
    // both methods are much faster than using zfs command
    fn from_mount_cmd(
        opt_alt_store: &Option<FilesystemType>,
    ) -> HttmResult<(BTreeMap<Arc<Path>, DatasetMetadata>, BTreeSet<Arc<Path>>)> {
        // do we have the necessary commands for search if user has not defined a snap point?
        // if so run the mount search, if not print some errors
        let mount_command = get_mount_command()?;

        let command_output = &ExecProcess::new(mount_command).output()?;

        let stderr_string = std::str::from_utf8(&command_output.stderr)?;

        if !stderr_string.is_empty() {
            return Err(HttmError::new(stderr_string).into());
        }

        let stdout_string = std::str::from_utf8(&command_output.stdout)?;

        // parse "mount" for filesystems and mountpoints
        let (map_of_datasets, filter_dirs): (
            BTreeMap<Arc<Path>, DatasetMetadata>,
            BTreeSet<Arc<Path>>,
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
                    opt_mount.map(|the_rest| (filesystem, the_rest.0, the_rest.1))
                // Busybox and BSD mount output
                } else if rest.contains(" (") {
                    let opt_mount = rest.split_once(" (");
                    opt_mount.map(|the_rest| (filesystem, the_rest.0, the_rest.1))
                } else {
                    None
                }
            })
            .map(|(filesystem, mount, the_rest)| {
                let link_type = if the_rest.contains(SMB_FSTYPE)
                    || the_rest.contains(AFP_FSTYPE)
                    || the_rest.contains(NFS_FSTYPE)
                {
                    LinkType::Network
                } else {
                    LinkType::Local
                };

                (PathBuf::from(filesystem), PathBuf::from(mount), link_type)
            })
            // sanity check: does the filesystem exist and have a ZFS hidden dir? if not, filter it out
            // and flip around, mount should key of key/value
            .partition_map(
                |(source, mount, link_type)| match FilesystemType::new(&mount) {
                    Some(FilesystemType::Zfs) => Either::Left((
                        Arc::from(mount),
                        DatasetMetadata {
                            source,
                            fs_type: FilesystemType::Zfs,
                            link_type,
                        },
                    )),
                    Some(FilesystemType::Btrfs(_)) => Either::Left((
                        Arc::from(mount),
                        DatasetMetadata {
                            source,
                            fs_type: FilesystemType::Btrfs(None),
                            link_type,
                        },
                    )),
                    _ if source.to_string_lossy().contains(RESTIC_FSTYPE) => {
                        let base_path = if let Some(FilesystemType::Restic(_)) = opt_alt_store {
                            mount
                        } else {
                            mount.join(RESTIC_LATEST_SNAPSHOT_DIRECTORY)
                        };

                        let canonical_path: PathBuf =
                            realpath(&base_path, RealpathFlags::ALLOW_MISSING)
                                .unwrap_or_else(|_| base_path.to_path_buf());

                        Either::Left((
                            Arc::from(canonical_path),
                            DatasetMetadata {
                                source,
                                fs_type: FilesystemType::Restic(None),
                                link_type,
                            },
                        ))
                    }
                    _ => Either::Right(Arc::from(mount)),
                },
            );

        Ok((map_of_datasets, filter_dirs))
    }

    pub fn from_blob_repo(
        map_of_datasets: &mut BTreeMap<Arc<Path>, DatasetMetadata>,
        repo_type: &FilesystemType,
    ) -> HttmResult<()> {
        map_of_datasets.retain(|_k, v| &v.fs_type == repo_type);

        let metadata = match repo_type {
            FilesystemType::Restic(_) => {
                if map_of_datasets.is_empty() {
                    return Err(HttmError::new(
                        "No supported Restic datasets were found on the system.",
                    )
                    .into());
                }

                let repos: Vec<PathBuf> = map_of_datasets.keys().map(|k| k.to_path_buf()).collect();

                DatasetMetadata {
                    source: PathBuf::from(RESTIC_FSTYPE),
                    fs_type: FilesystemType::Restic(Some(Box::new(ResticAdditionalData { repos }))),
                    link_type: LinkType::Local,
                }
            }
            FilesystemType::Apfs => {
                if !cfg!(target_os = "macos") {
                    return Err(HttmError::new(
                                    "Time Machine is only supported on Mac OS.  This appears to be an unsupported OS."
                                )
                                .into());
                }

                if !TM_DIR_REMOTE_PATH.exists() && !TM_DIR_LOCAL_PATH.exists() {
                    return Err(HttmError::new(
                                    "Neither a local nor a remote Time Machine path seems to exist for this system."
                                )
                                .into());
                }

                DatasetMetadata {
                    source: PathBuf::from("timemachine"),
                    fs_type: FilesystemType::Apfs,
                    link_type: LinkType::Local,
                }
            }
            _ => {
                return Err(HttmError::new(
                    "The file system type specified is not a supported alternative store.",
                )
                .into());
            }
        };

        let mut new = BTreeMap::new();

        new.insert(Arc::from(ROOT_PATH.as_ref()), metadata);

        *map_of_datasets = new;

        return Ok(());
    }

    // if we have some btrfs mounts, we check to see if there is a snap directory in common
    // so we can hide that common path from searches later
    pub fn common_snap_dir(&self) -> Option<PathBuf> {
        let map_of_datasets: &MapOfDatasets = &self.map_of_datasets;
        let map_of_snaps: &MapOfSnaps = &self.map_of_snaps;

        if map_of_datasets
            .par_iter()
            .any(|(_mount, dataset_info)| !matches!(dataset_info.fs_type, FilesystemType::Zfs))
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
