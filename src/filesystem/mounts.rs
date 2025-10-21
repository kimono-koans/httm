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

use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::get_mount_command;
use crate::{
    BACKUPS_OF, BTRFS_SNAPPER_HIDDEN_DIRECTORY, GLOBAL_CONFIG, NILFS2_SNAPSHOT_ID_KEY,
    RESTIC_LATEST_SNAPSHOT_DIRECTORY, TM_DIR_LOCAL, TM_DIR_REMOTE, ZFS_HIDDEN_DIRECTORY,
    ZFS_SNAPSHOT_DIRECTORY,
};
use itertools::Either;
use itertools::Itertools;
use proc_mounts::MountIter;
use realpath_ext::{RealpathFlags, realpath};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkType {
    Local,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtrfsAdditionalData {
    pub base_subvol: Box<Path>,
    pub snap_names: OnceLock<BTreeMap<Box<Path>, Box<Path>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResticAdditionalData {
    pub repos: Vec<Box<Path>>,
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
    pub source: Box<Path>,
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
        *LazyLock::new(|| {
            self.inner
                .iter()
                .map(|dir| dir.components().count())
                .max()
                .unwrap_or(usize::MAX)
        })
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

impl From<BTreeMap<Arc<Path>, DatasetMetadata>> for MapOfDatasets {
    fn from(value: BTreeMap<Arc<Path>, DatasetMetadata>) -> Self {
        Self { inner: value }
    }
}

impl MaxLen for MapOfDatasets {
    fn max_len(&self) -> usize {
        *LazyLock::new(|| {
            self.inner
                .keys()
                .map(|mount| mount.components().count())
                .max()
                .unwrap_or(usize::MAX)
        })
    }
}

pub static PROC_MOUNTS: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/proc/mounts"));
pub static BTRFS_ROOT_SUBVOL: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("<FS_TREE>"));
pub static ROOT_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/"));
static ETC_MNT_TAB: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/etc/mnttab"));
pub static TM_DIR_REMOTE_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from(TM_DIR_REMOTE));
pub static TM_DIR_LOCAL_PATH: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from(TM_DIR_LOCAL));

pub struct BaseFilesystemInfo {
    pub map_of_datasets: MapOfDatasets,
    pub filter_dirs: FilterDirs,
}

impl BaseFilesystemInfo {
    // divide by the type of system we are on
    // Linux allows us the read proc mounts
    pub fn new(opt_alt_store: &Option<FilesystemType>) -> HttmResult<Self> {
        let (raw_datasets, mut filter_dirs_set) = if PROC_MOUNTS.exists() {
            Self::from_file(&PROC_MOUNTS, opt_alt_store)?
        } else if ETC_MNT_TAB.exists() {
            Self::from_file(&ETC_MNT_TAB, opt_alt_store)?
        } else {
            Self::from_mount_cmd(opt_alt_store)?
        };

        let map_of_datasets = {
            MapOfDatasets {
                inner: raw_datasets,
            }
        };

        #[cfg(target_os = "macos")]
        {
            filter_dirs_set.insert(Arc::from(TM_DIR_LOCAL_PATH.as_path()));
            filter_dirs_set.insert(Arc::from(TM_DIR_REMOTE_PATH.as_path()));
        }

        let filter_dirs = {
            FilterDirs {
                inner: filter_dirs_set,
            }
        };

        Ok(BaseFilesystemInfo {
            map_of_datasets,
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
                let dest_path = Arc::from(Path::new(&mount_info.dest));
                (mount_info, dest_path)
            })
            .partition_map(|(mount_info, dest_path)| match mount_info.fstype.as_str() {
                ZFS_FSTYPE => Either::Left((
                    dest_path,
                    DatasetMetadata {
                        source: mount_info.source.into_boxed_path(),
                        fs_type: FilesystemType::Zfs,
                        link_type: LinkType::Local,
                    },
                )),
                SMB_FSTYPE | AFP_FSTYPE | NFS_FSTYPE => match FilesystemType::new(&dest_path) {
                    Some(FilesystemType::Zfs) => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: mount_info.source.into_boxed_path(),
                            fs_type: FilesystemType::Zfs,
                            link_type: LinkType::Network,
                        },
                    )),
                    Some(FilesystemType::Btrfs(None)) => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: mount_info.source.into_boxed_path(),
                            fs_type: FilesystemType::Btrfs(None),
                            link_type: LinkType::Network,
                        },
                    )),
                    _ => Either::Right(dest_path),
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
                            Some(id) if *id == "5" => BTRFS_ROOT_SUBVOL.as_path(),
                            _ => Path::new(subvol),
                        })
                        .map(|base_subvol| {
                            Box::new(BtrfsAdditionalData {
                                base_subvol: base_subvol.into(),
                                snap_names: OnceLock::new(),
                            })
                        });

                    Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: mount_info.source.into_boxed_path(),
                            fs_type: FilesystemType::Btrfs(opt_additional_data),
                            link_type: LinkType::Local,
                        },
                    ))
                }
                NILFS2_FSTYPE => Either::Left((
                    dest_path,
                    DatasetMetadata {
                        source: mount_info.source.into_boxed_path(),
                        fs_type: FilesystemType::Nilfs2,
                        link_type: LinkType::Local,
                    },
                )),
                _ if mount_info.source.to_string_lossy().contains(RESTIC_FSTYPE) => {
                    let base_path = if let Some(FilesystemType::Restic(_)) = opt_alt_store {
                        dest_path.to_path_buf()
                    } else {
                        dest_path.as_ref().join(RESTIC_LATEST_SNAPSHOT_DIRECTORY)
                    };

                    let canonical_path = realpath(&base_path, RealpathFlags::ALLOW_MISSING)
                        .unwrap_or_else(|_| base_path.to_path_buf())
                        .into();

                    Either::Left((
                        canonical_path,
                        DatasetMetadata {
                            source: mount_info.source.into_boxed_path(),
                            fs_type: FilesystemType::Restic(None),
                            link_type: LinkType::Local,
                        },
                    ))
                }
                _ => Either::Right(dest_path),
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
            return HttmError::new(stderr_string).into();
        }

        let stdout_string = std::str::from_utf8(&command_output.stdout)?;

        // parse "mount" for filesystems and mountpoints
        let (map_of_datasets, filter_dirs): (
            BTreeMap<Arc<Path>, DatasetMetadata>,
            BTreeSet<Arc<Path>>,
        ) = stdout_string
            .lines()
            // but exclude snapshot mounts.  we want the raw filesystem names.
            .filter(|line| !line.contains(ZFS_HIDDEN_DIRECTORY))
            .filter(|line| !line.contains(TM_DIR_REMOTE))
            .filter(|line| !line.contains(TM_DIR_LOCAL))
            .filter(|line| !line.contains(BACKUPS_OF))
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

                (
                    Box::from(Path::new(filesystem)),
                    Arc::from(Path::new(mount)),
                    link_type,
                )
            })
            // sanity check: does the filesystem exist and have a ZFS hidden dir? if not, filter it out
            // and flip around, mount should key of key/value
            .partition_map(
                |(source, mount, link_type)| match FilesystemType::new(&mount) {
                    Some(FilesystemType::Zfs) => Either::Left((
                        mount,
                        DatasetMetadata {
                            source,
                            fs_type: FilesystemType::Zfs,
                            link_type,
                        },
                    )),
                    Some(FilesystemType::Btrfs(_)) => Either::Left((
                        mount,
                        DatasetMetadata {
                            source,
                            fs_type: FilesystemType::Btrfs(None),
                            link_type,
                        },
                    )),
                    _ if source.to_string_lossy().contains(RESTIC_FSTYPE) => {
                        let base_path = if let Some(FilesystemType::Restic(_)) = opt_alt_store {
                            mount.to_path_buf()
                        } else {
                            mount.join(RESTIC_LATEST_SNAPSHOT_DIRECTORY)
                        };

                        let canonical_path = realpath(&base_path, RealpathFlags::ALLOW_MISSING)
                            .unwrap_or_else(|_| base_path.to_path_buf())
                            .into();

                        Either::Left((
                            canonical_path,
                            DatasetMetadata {
                                source,
                                fs_type: FilesystemType::Restic(None),
                                link_type,
                            },
                        ))
                    }
                    _ => Either::Right(mount),
                },
            );

        Ok((map_of_datasets, filter_dirs))
    }

    pub fn from_blob_repo(&mut self, repo_type: &FilesystemType) -> HttmResult<()> {
        let metadata = match repo_type {
            FilesystemType::Restic(_) => {
                let retained_keys: Vec<Box<Path>> = self
                    .map_of_datasets
                    .iter()
                    .filter(|(_k, v)| &v.fs_type == repo_type)
                    .map(|(k, _v)| k.as_ref().into())
                    .collect();

                if retained_keys.is_empty() {
                    return HttmError::new(
                        "No supported Restic datasets were found on the system.",
                    )
                    .into();
                }

                let repos: Vec<Box<Path>> = retained_keys;

                DatasetMetadata {
                    source: Path::new(RESTIC_FSTYPE).into(),
                    fs_type: FilesystemType::Restic(Some(Box::new(ResticAdditionalData { repos }))),
                    link_type: LinkType::Local,
                }
            }
            FilesystemType::Apfs => {
                if !cfg!(target_os = "macos") {
                    return HttmError::new(
                                    "Time Machine is only supported on Mac OS.  This appears to be an unsupported OS."
                                )
                                .into();
                }

                if !TM_DIR_REMOTE_PATH.exists() && !TM_DIR_LOCAL_PATH.exists() {
                    return HttmError::new(
                                    "Neither a local nor a remote Time Machine path seems to exist for this system."
                                )
                                .into();
                }

                DatasetMetadata {
                    source: Path::new("timemachine").into(),
                    fs_type: FilesystemType::Apfs,
                    link_type: LinkType::Local,
                }
            }
            _ => {
                return HttmError::new(
                    "The file system type specified is not a supported alternative store.",
                )
                .into();
            }
        };

        let datasets = BTreeMap::from([(Arc::from(ROOT_PATH.as_ref()), metadata)]);

        *self = Self {
            map_of_datasets: datasets.into(),
            filter_dirs: self.filter_dirs.clone(),
        };

        Ok(())
    }
}
