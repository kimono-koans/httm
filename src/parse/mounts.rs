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
use rayon::iter::Either;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::Read;
use std::ops::Deref;
use std::path::PathBuf;
use std::process::Command as ExecProcess;
use which::which;

pub const ZFS_FSTYPE: &str = "zfs";
pub const NILFS2_FSTYPE: &str = "nilfs2";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilesystemType {
    Zfs,
    Btrfs,
    Nilfs2,
    Apfs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MountType {
    Local,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetMetadata {
    pub source: PathBuf,
    pub fs_type: FilesystemType,
    pub mount_type: MountType,
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
// skipping dump and pass as not needed for our purposes
pub struct MountInfo<'a> {
    pub source: &'a str,
    pub dest: &'a str,
    pub fs_type: &'a str,
    pub opts: Vec<&'a str>,
}

impl<'a> MountInfo<'a> {
    pub fn new(line: &'a str, separator: &str) -> Option<Self> {
        let split_line: Vec<&str> = line.split(separator).collect();

        if split_line.len() < 4 {
            return None;
        }

        let opts: Vec<&str> = split_line[3].split(',').collect();

        Some(MountInfo {
            source: split_line[0],
            dest: split_line[1],
            fs_type: split_line[2],
            opts,
        })
    }
}

pub struct MountBuffer<'a> {
    pub buffer: String,
    pub mount_file: &'a MountFile,
}

impl<'a> Deref for MountBuffer<'a> {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl<'a> MountBuffer<'a> {
    pub fn new(mount_file: &'a MountFile) -> HttmResult<Self> {
        let file = std::fs::File::open(&mount_file.path)?;

        let mut reader = std::io::BufReader::new(file);
        let mut buffer = String::new();

        reader.read_to_string(&mut buffer)?;

        Ok(Self { buffer, mount_file })
    }

    pub fn process_line(&self, line: &'a str) -> Option<MountInfo<'a>> {
        MountInfo::new(line, &self.mount_file.separator)
    }
}

pub struct MountFile {
    path: PathBuf,
    separator: String,
}

pub static PROC_MOUNTS: Lazy<MountFile> = Lazy::new(|| MountFile {
    path: PathBuf::from("/proc/mounts"),
    separator: ' '.to_string(),
});

static ETC_MNTTAB: Lazy<MountFile> = Lazy::new(|| MountFile {
    path: PathBuf::from("/proc/mounts"),
    separator: '\t'.to_string(),
});

pub struct BaseFilesystemInfo {
    pub map_of_datasets: MapOfDatasets,
    pub map_of_snaps: MapOfSnaps,
    pub filter_dirs: FilterDirs,
}

impl BaseFilesystemInfo {
    // divide by the type of system we are on
    // Linux allows us the read proc mounts
    pub fn new() -> HttmResult<Self> {
        let (raw_datasets, filter_dirs_set) = if PROC_MOUNTS.path.exists() {
            Self::from_file(&PROC_MOUNTS)?
        } else if ETC_MNTTAB.path.exists() {
            Self::from_file(&ETC_MNTTAB)?
        } else {
            Self::from_mount_cmd()?
        };

        let map_of_snaps = MapOfSnaps::new(&raw_datasets)?;

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
        mount_file: &MountFile,
    ) -> HttmResult<(HashMap<PathBuf, DatasetMetadata>, HashSet<PathBuf>)> {
        let buffer = MountBuffer::new(&mount_file)?;

        let (map_of_datasets, filter_dirs): (HashMap<PathBuf, DatasetMetadata>, HashSet<PathBuf>) =
            buffer
                .par_lines()
                .filter_map(|line| buffer.process_line(line))
                // but exclude snapshot mounts.  we want only the raw filesystems
                .filter(|mount_info| match mount_info.fs_type {
                    ZFS_FSTYPE if mount_info.dest.contains(ZFS_HIDDEN_DIRECTORY) => false,
                    NILFS2_FSTYPE
                        if mount_info
                            .opts
                            .iter()
                            .any(|opt| opt.contains(NILFS2_SNAPSHOT_ID_KEY)) =>
                    {
                        false
                    }
                    _ => true,
                })
                .map(|mount_info| {
                    let dest_path = PathBuf::from(&mount_info.dest);
                    (mount_info, dest_path)
                })
                .partition_map(|(mount_info, dest_path)| match mount_info.fs_type {
                    ZFS_FSTYPE => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Zfs,
                            mount_type: MountType::Local,
                        },
                    )),
                    SMB_FSTYPE | AFP_FSTYPE | NFS_FSTYPE => {
                        match fs_type_from_hidden_dir(&dest_path) {
                            Some(FilesystemType::Zfs) => Either::Left((
                                dest_path,
                                DatasetMetadata {
                                    source: PathBuf::from(mount_info.source),
                                    fs_type: FilesystemType::Zfs,
                                    mount_type: MountType::Network,
                                },
                            )),
                            Some(FilesystemType::Btrfs) => Either::Left((
                                dest_path,
                                DatasetMetadata {
                                    source: PathBuf::from(mount_info.source),
                                    fs_type: FilesystemType::Btrfs,
                                    mount_type: MountType::Network,
                                },
                            )),
                            _ => Either::Right(dest_path),
                        }
                    }
                    BTRFS_FSTYPE => {
                        let keyed_options: BTreeMap<&str, &str> = mount_info
                            .opts
                            .iter()
                            .filter(|line| line.contains('='))
                            .filter_map(|line| line.split_once('='))
                            .collect();

                        let source = match keyed_options.get("subvol") {
                            Some(subvol) => PathBuf::from(subvol),
                            None => PathBuf::from(mount_info.source),
                        };

                        Either::Left((
                            dest_path,
                            DatasetMetadata {
                                source,
                                fs_type: FilesystemType::Btrfs,
                                mount_type: MountType::Local,
                            },
                        ))
                    }
                    NILFS2_FSTYPE => Either::Left((
                        dest_path,
                        DatasetMetadata {
                            source: PathBuf::from(mount_info.source),
                            fs_type: FilesystemType::Nilfs2,
                            mount_type: MountType::Local,
                        },
                    )),
                    _ => Either::Right(dest_path),
                });

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, filter_dirs))
        }
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
                        mount_type: MountType::Local,
                    },
                )),
                Some(FilesystemType::Btrfs) => Either::Left((
                    mount,
                    DatasetMetadata {
                        source,
                        fs_type: FilesystemType::Btrfs,
                        mount_type: MountType::Local,
                    },
                )),
                _ => Either::Right(mount),
            });

        Self::from_tm_dir(&mut map_of_datasets);

        if map_of_datasets.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok((map_of_datasets, filter_dirs))
        }
    }

    fn from_tm_dir(map_of_datasets: &mut HashMap<PathBuf, DatasetMetadata>) {
        if cfg!(target_os = "macos") {
            let tm_dir_remote_path = std::path::Path::new(TM_DIR_REMOTE);
            let tm_dir_local_path = std::path::Path::new(TM_DIR_LOCAL);

            if tm_dir_remote_path.exists() || tm_dir_local_path.exists() {
                let root_dir = PathBuf::from(ROOT_DIRECTORY);

                match map_of_datasets.get(&root_dir) {
                    Some(md) => {
                        eprintln!("WARN: httm has prioritized a discovered root directory mount over any potential Time Machine mounts: {:?}", md.source);
                    }
                    None => {
                        let metadata = DatasetMetadata {
                            source: PathBuf::from("timemachine"),
                            fs_type: FilesystemType::Apfs,
                            mount_type: MountType::Local,
                        };

                        // SAFETY: Check no entry is here just above
                        map_of_datasets.insert_unique_unchecked(root_dir, metadata);
                    }
                }
            }
        }
    }

    // if we have some btrfs mounts, we check to see if there is a snap directory in common
    // so we can hide that common path from searches later
    pub fn common_snap_dir(&self) -> Option<PathBuf> {
        let map_of_datasets: &MapOfDatasets = &self.map_of_datasets;
        let map_of_snaps: &MapOfSnaps = &self.map_of_snaps;

        if map_of_datasets
            .par_iter()
            .any(|(_mount, dataset_info)| dataset_info.fs_type == FilesystemType::Btrfs)
        {
            let vec_snaps: Vec<&PathBuf> = map_of_datasets
                .par_iter()
                .filter(|(_mount, dataset_info)| {
                    if dataset_info.fs_type == FilesystemType::Btrfs {
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
