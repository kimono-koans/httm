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

use crate::background::recursive::PathProvenance;
use crate::config::generate::{ListSnapsOfType, PrintMode};
use crate::library::file_ops::HashFileContents;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{date_string, display_human_size, DateFormat};
use crate::parse::mounts::FilesystemType;
use crate::parse::mounts::MaxLen;
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};
use realpath_ext::{realpath, RealpathFlags};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::cmp::{Ord, Ordering, PartialOrd};
use std::ffi::OsStr;
use std::fs::{symlink_metadata, DirEntry, FileType, Metadata};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::OnceLock;
use std::time::SystemTime;

use super::selection::SelectionCandidate;

static DATASET_MAX_LEN: LazyLock<usize> =
    LazyLock::new(|| GLOBAL_CONFIG.dataset_collection.map_of_datasets.max_len());

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct BasicDirEntryInfo {
    path: PathBuf,
    opt_filetype: Option<FileType>,
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            path: dir_entry.path(),
            opt_filetype: dir_entry.file_type().ok(),
        }
    }
}

impl BasicDirEntryInfo {
    pub fn new(path: PathBuf, opt_filetype: Option<FileType>) -> Self {
        Self { path, opt_filetype }
    }

    pub fn filename(&self) -> &OsStr {
        self.path.file_name().unwrap_or_default()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn opt_filetype(&self) -> &Option<FileType> {
        &self.opt_filetype
    }

    pub fn to_path_buf(self) -> PathBuf {
        self.path
    }

    pub fn into_selection(self, is_phantom: &PathProvenance) -> SelectionCandidate {
        let mut selection: SelectionCandidate = self.into();
        selection.set_phantom(is_phantom);
        selection
    }
}

impl Into<SelectionCandidate> for BasicDirEntryInfo {
    fn into(self) -> SelectionCandidate {
        SelectionCandidate::new(self.path, self.opt_filetype)
    }
}

pub trait PathDeconstruction<'a> {
    fn alias(&self) -> Option<AliasedPath>;
    fn target(&self, proximate_dataset_mount: &Path) -> Option<PathBuf>;
    fn source(&self, opt_proximate_dataset_mount: Option<&'a Path>) -> Option<PathBuf>;
    fn fs_type(&self, opt_proximate_dataset_mount: Option<&'a Path>) -> Option<FilesystemType>;
    fn relative_path(&'a self, proximate_dataset_mount: &'a Path) -> HttmResult<&'a Path>;
    fn proximate_dataset(&'a self) -> HttmResult<&'a Path>;
    fn live_path(&self) -> Option<PathBuf>;
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    path_buf: PathBuf,
    metadata: Option<PathMetadata>,
}

impl PartialOrd for PathData {
    #[inline]
    fn partial_cmp(&self, other: &PathData) -> Option<Ordering> {
        Some(self.path().cmp(&other.path()))
    }
}

impl Ord for PathData {
    #[inline]
    fn cmp(&self, other: &PathData) -> Ordering {
        self.path().cmp(&other.path())
    }
}

impl<T: AsRef<Path>> From<T> for PathData {
    fn from(path: T) -> Self {
        // this metadata() function will not traverse symlinks
        let opt_metadata = symlink_metadata(path.as_ref()).ok();
        PathData::new(path.as_ref(), opt_metadata)
    }
}

// don't use new(), because DirEntry includes the canonical path
// saves a few stat/md calls
impl From<BasicDirEntryInfo> for PathData {
    fn from(basic_info: BasicDirEntryInfo) -> Self {
        // this metadata() function will not traverse symlinks
        let opt_metadata = basic_info.path.symlink_metadata().ok();
        let path = basic_info.path;
        Self::new(&path, opt_metadata)
    }
}

impl PathData {
    #[inline(always)]
    pub fn new(path: &Path, opt_metadata: Option<Metadata>) -> Self {
        // canonicalize() on any path that DNE will throw an error
        //
        // in general we handle those cases elsewhere, like the ingest
        // of input files in Config::from for deleted relative paths, etc.
        let canonical_path: PathBuf =
            realpath(path, RealpathFlags::ALLOW_MISSING).unwrap_or_else(|_| path.to_path_buf());

        let path_metadata = opt_metadata.and_then(|md| PathMetadata::new(&md));

        Self {
            path_buf: canonical_path,
            metadata: path_metadata,
        }
    }

    pub fn path<'a>(&'a self) -> &'a Path {
        &self.path_buf
    }

    pub fn opt_metadata<'a>(&'a self) -> &'a Option<PathMetadata> {
        &self.metadata
    }

    #[inline(always)]
    pub fn metadata_infallible(&self) -> PathMetadata {
        self.metadata.unwrap_or_else(|| PHANTOM_PATH_METADATA)
    }

    pub fn is_same_file_contents(&self, other: &Self) -> bool {
        let self_hash = HashFileContents::path_to_hash(self.path());
        let other_hash = HashFileContents::path_to_hash(other.path());

        self_hash.cmp(&other_hash) == Ordering::Equal
    }
}

impl<'a> PathDeconstruction<'a> for PathData {
    fn alias(&self) -> Option<AliasedPath> {
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_aliases
            .as_ref()
            .and_then(|map_of_aliases| {
                self.path_buf.ancestors().find_map(|ancestor| {
                    map_of_aliases.get(ancestor).and_then(|metadata| {
                        Some(AliasedPath {
                            proximate_dataset: metadata.remote_dir.as_ref(),
                            relative_path: &self.path_buf.strip_prefix(ancestor).ok()?,
                        })
                    })
                })
            })
    }

    fn live_path(&self) -> Option<PathBuf> {
        Some(self.path_buf.clone())
    }

    #[inline(always)]
    fn relative_path(&'a self, proximate_dataset_mount: &Path) -> HttmResult<&'a Path> {
        // path strip, if aliased
        // fallback if unable to find an alias or strip a prefix
        // (each an indication we should not be trying aliases)
        self.path_buf
            .strip_prefix(proximate_dataset_mount)
            .map_err(|err| err.into())
    }

    fn target(&self, proximate_dataset_mount: &Path) -> Option<PathBuf> {
        Some(proximate_dataset_mount.to_path_buf())
    }

    fn source(&self, opt_proximate_dataset_mount: Option<&'a Path>) -> Option<PathBuf> {
        let mount: &Path =
            opt_proximate_dataset_mount.map_or_else(|| self.proximate_dataset().ok(), Some)?;

        GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(mount)
            .map(|md| md.source.clone())
    }

    #[inline(always)]
    fn proximate_dataset(&'a self) -> HttmResult<&'a Path> {
        // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
        // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
        // we map to return the key, instead of the value
        self.path_buf
            .ancestors()
            .skip_while(|ancestor| ancestor.components().count() > *DATASET_MAX_LEN)
            .find(|ancestor| {
                GLOBAL_CONFIG
                    .dataset_collection
                    .map_of_datasets
                    .contains_key(*ancestor)
            })
            .ok_or_else(|| {
                let msg = format!(
                    "httm could not identify any proximate dataset for path: {:?}",
                    self.path_buf
                );
                HttmError::new(&msg).into()
            })
    }

    fn fs_type(&self, opt_proximate_dataset_mount: Option<&'a Path>) -> Option<FilesystemType> {
        let proximate_dataset =
            opt_proximate_dataset_mount.map_or_else(|| self.proximate_dataset().ok(), Some)?;

        GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(proximate_dataset)
            .map(|md| md.fs_type.clone())
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AliasedPath<'a> {
    pub proximate_dataset: &'a Path,
    pub relative_path: &'a Path,
}

pub struct ZfsSnapPathGuard<'a> {
    inner: &'a PathData,
}

impl<'a> std::ops::Deref for ZfsSnapPathGuard<'a> {
    type Target = &'a PathData;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> ZfsSnapPathGuard<'a> {
    pub fn new(pathdata: &'a PathData) -> Option<Self> {
        if !Self::is_zfs_snap_path(pathdata) {
            return None;
        }

        Some(Self { inner: pathdata })
    }

    pub fn is_zfs_snap_path(pathdata: &'a PathData) -> bool {
        pathdata
            .path_buf
            .to_string_lossy()
            .contains(ZFS_SNAPSHOT_DIRECTORY)
    }
}

impl<'a> PathDeconstruction<'a> for ZfsSnapPathGuard<'_> {
    fn alias(&self) -> Option<AliasedPath> {
        // aliases aren't allowed for snap paths
        None
    }

    fn live_path(&self) -> Option<PathBuf> {
        self.inner
            .path_buf
            .to_string_lossy()
            .split_once(&format!("{ZFS_SNAPSHOT_DIRECTORY}/"))
            .and_then(|(proximate_dataset_mount, relative_and_snap_name)| {
                relative_and_snap_name
                    .split_once("/")
                    .map(|(_snap_name, relative)| {
                        PathBuf::from(proximate_dataset_mount).join(Path::new(relative))
                    })
            })
    }

    fn target(&self, proximate_dataset_mount: &Path) -> Option<PathBuf> {
        self.relative_path(proximate_dataset_mount)
            .ok()
            .map(|relative| {
                self.inner
                    .path_buf
                    .ancestors()
                    .zip(relative.ancestors())
                    .skip_while(|(a_path, b_path)| a_path == b_path)
                    .map(|(a_path, _b_path)| a_path)
                    .collect()
            })
    }

    fn relative_path(&'a self, proximate_dataset_mount: &'a Path) -> HttmResult<&'a Path> {
        let relative_path = self.inner.relative_path(proximate_dataset_mount)?;
        let snapshot_stripped_set = relative_path.strip_prefix(ZFS_SNAPSHOT_DIRECTORY)?;

        snapshot_stripped_set
            .components()
            .next()
            .and_then(|snapshot_name| snapshot_stripped_set.strip_prefix(snapshot_name).ok())
            .ok_or_else(|| {
                let msg = format!(
                    "httm could not identify any relative path for path: {:?}",
                    self.path_buf
                );
                HttmError::new(&msg).into()
            })
    }

    fn source(&self, _opt_proximate_dataset_mount: Option<&'a Path>) -> Option<PathBuf> {
        let path_string = &self.inner.path_buf.to_string_lossy();

        let (dataset_path, relative_and_snap) =
            path_string.split_once(&format!("{ZFS_SNAPSHOT_DIRECTORY}/"))?;

        let (snap_name, _relative) = relative_and_snap
            .split_once('/')
            .unwrap_or_else(|| (relative_and_snap, ""));

        match GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(Path::new(dataset_path))
        {
            Some(md) if md.fs_type == FilesystemType::Zfs => {
                let res = format!("{}@{snap_name}", md.source.to_string_lossy());
                Some(PathBuf::from(res))
            }
            Some(_md) => {
                eprintln!("WARN: {:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.", self.inner.path_buf);
                None
            }
            _ => {
                eprintln!("WARN: {:?} is not located on a discoverable dataset.  httm can only list snapshot names for ZFS datasets.", self.inner.path_buf);
                None
            }
        }
    }

    fn proximate_dataset(&'a self) -> HttmResult<&'a Path> {
        self.inner.proximate_dataset()
    }

    fn fs_type(&self, _opt_proximate_dataset_mount: Option<&'a Path>) -> Option<FilesystemType> {
        Some(FilesystemType::Zfs)
    }
}

impl Serialize for PathData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PathData", 2)?;

        state.serialize_field("path", &self.path_buf)?;
        state.serialize_field("metadata", &self.metadata)?;
        state.end()
    }
}

impl Serialize for PathMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PathData", 2)?;

        if matches!(
            GLOBAL_CONFIG.print_mode,
            PrintMode::RawNewline | PrintMode::RawZero
        ) {
            state.serialize_field("size", &self.size)?;
            state.serialize_field("modify_time", &self.modify_time)?;
        } else {
            let size = display_human_size(self.size);
            let date = date_string(
                GLOBAL_CONFIG.requested_utc_offset,
                &self.modify_time,
                DateFormat::Display,
            );

            state.serialize_field("size", &size)?;
            state.serialize_field("modify_time", &date)?;
        }

        state.end()
    }
}

#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathMetadata {
    size: u64,
    modify_time: SystemTime,
}

impl PathMetadata {
    // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
    #[inline(always)]
    pub fn new(md: &Metadata) -> Option<Self> {
        // may fail on systems that don't collect a modify time
        md.modified().ok().map(|time| PathMetadata {
            size: md.len(),
            modify_time: time,
        })
    }

    // using ctime instead of mtime might be more correct as mtime can be trivially changed from user space
    // but I think we want to use mtime here? People should be able to make a snapshot "unique" with only mtime?
    #[inline(always)]
    pub fn mtime(&self) -> SystemTime {
        self.modify_time
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

pub const PHANTOM_DATE: SystemTime = SystemTime::UNIX_EPOCH;
pub const PHANTOM_SIZE: u64 = 0u64;

pub const PHANTOM_PATH_METADATA: PathMetadata = PathMetadata {
    size: PHANTOM_SIZE,
    modify_time: PHANTOM_DATE,
};

#[derive(Eq, PartialEq)]
pub struct CompareVersionsContainer<'a> {
    pathdata: &'a PathData,
    opt_hash: Option<OnceLock<u64>>,
}

impl<'a> PartialOrd for CompareVersionsContainer<'a> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for CompareVersionsContainer<'a> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        let self_md = self.pathdata.metadata_infallible();
        let other_md = other.pathdata.metadata_infallible();

        if self_md.modify_time == other_md.modify_time {
            return self_md.size.cmp(&other_md.size);
        }

        // if files, differ re mtime, but have same size, we test by bytes whether the same
        if self_md.size == other_md.size
            && self.opt_hash.is_some()
            // if above is true/false then "&& other.opt_hash.is_some()" is the same
            && self.is_same_file_contents(other)
        {
            return Ordering::Equal;
        }

        self_md.modify_time.cmp(&other_md.modify_time)
    }
}

impl<'a> CompareVersionsContainer<'a> {
    #[inline(always)]
    pub fn new(pathdata: &'a PathData, snaps_of_type: &ListSnapsOfType) -> Self {
        let opt_hash = match snaps_of_type {
            ListSnapsOfType::UniqueContents => Some(OnceLock::new()),
            ListSnapsOfType::UniqueMetadata | ListSnapsOfType::All => None,
        };

        CompareVersionsContainer { pathdata, opt_hash }
    }

    #[allow(unused_assignments)]
    pub fn is_same_file_contents(&self, other: &Self) -> bool {
        // SAFETY: Unwrap will fail on opt_hash is None, here we've guarded this above
        let self_hash_cell = self
            .opt_hash
            .as_ref()
            .expect("opt_hash should be check prior to this point and must be Some");
        let other_hash_cell = other
            .opt_hash
            .as_ref()
            .expect("opt_hash should be check prior to this point and must be Some");

        let (self_hash, other_hash): (HttmResult<u64>, HttmResult<u64>) = rayon::join(
            || {
                if let Some(hash_value) = self_hash_cell.get() {
                    return Ok(*hash_value);
                };

                Ok(*self_hash_cell
                    .get_or_init(|| HashFileContents::path_to_hash(self.pathdata.path())))
            },
            || {
                if let Some(hash_value) = other_hash_cell.get() {
                    return Ok(*hash_value);
                }

                Ok(*other_hash_cell
                    .get_or_init(|| HashFileContents::path_to_hash(other.pathdata.path())))
            },
        );

        if let Ok(res_self) = self_hash {
            if let Ok(res_other) = other_hash {
                return res_self == res_other;
            }
        }

        false
    }
}
