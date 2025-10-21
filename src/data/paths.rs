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
use crate::config::generate::{DedupBy, PrintMode};
use crate::data::selection::SelectionCandidate;
use crate::filesystem::mounts::{FilesystemType, IsFilterDir, MaxLen};
use crate::library::file_ops::ChecksumFileContents;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::UniqueInode;
use crate::library::utility::dir_was_previously_listed;
use crate::library::utility::{DateFormat, HttmIsDir, date_string, display_human_size};
use crate::library::utility::{ENV_LS_COLORS, PaintString};
use crate::{
    BTRFS_SNAPPER_HIDDEN_DIRECTORY, GLOBAL_CONFIG, MAC_OS_HIDDEN_DIRS, OPT_COMMON_SNAP_DIR,
    ZFS_HIDDEN_DIRECTORY, ZFS_SNAPSHOT_DIRECTORY,
};
use hashbrown::HashSet;
use lscolors::Colorable;
use realpath_ext::{RealpathFlags, realpath};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::cell::RefCell;
use std::cmp::{Ord, Ordering, PartialOrd};
use std::ffi::OsStr;
use std::fs::{DirEntry, FileType, Metadata};
use std::hash::Hash;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::SystemTime;

static OPT_REQUESTED_DIR_DEV: LazyLock<u64> = LazyLock::new(|| {
    GLOBAL_CONFIG
        .opt_requested_dir
        .as_ref()
        .expect("opt_requested_dir should be Some value at this point in execution")
        .symlink_metadata()
        .expect("Cannot read metadata for directory requested for search.")
        .dev()
});

static DATASET_MAX_LEN: LazyLock<usize> =
    LazyLock::new(|| GLOBAL_CONFIG.dataset_collection.map_of_datasets.max_len());

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug)]
pub struct BasicDirEntryInfo {
    path: Box<Path>,
    opt_filetype: Option<FileType>,
    opt_dir_entry: Option<DirEntry>,
    opt_metadata: OnceLock<Option<Metadata>>,
}

impl Clone for BasicDirEntryInfo {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            opt_filetype: self.opt_filetype.clone(),
            opt_dir_entry: None,
            opt_metadata: OnceLock::new(),
        }
    }
}

impl Hash for BasicDirEntryInfo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl PartialEq for BasicDirEntryInfo {
    fn eq(&self, other: &Self) -> bool {
        self.path.eq(&other.path)
    }
}

impl Eq for BasicDirEntryInfo {}

impl From<DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: DirEntry) -> Self {
        BasicDirEntryInfo {
            path: dir_entry.path().into_boxed_path(),
            opt_filetype: dir_entry.file_type().ok(),
            opt_dir_entry: Some(dir_entry),
            opt_metadata: OnceLock::new(),
        }
    }
}

impl Colorable for BasicDirEntryInfo {
    fn path(&self) -> PathBuf {
        self.path().to_path_buf()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.opt_dir_entry
            .as_ref()
            .map(|de| de.file_name())
            .unwrap_or_default()
            .to_os_string()
    }
    fn file_type(&self) -> Option<FileType> {
        self.opt_filetype().copied()
    }
    fn metadata(&self) -> Option<std::fs::Metadata> {
        self.opt_metadata().cloned()
    }
}

impl BasicDirEntryInfo {
    pub fn new(path: &Path, opt_filetype: Option<FileType>) -> Self {
        Self {
            path: path.into(),
            opt_filetype: opt_filetype
                .or_else(|| path.symlink_metadata().ok().map(|md| md.file_type())),
            opt_dir_entry: None,
            opt_metadata: OnceLock::new(),
        }
    }

    pub fn into_selection_candidate(self, path_provenance: &PathProvenance) -> SelectionCandidate {
        let opt_metadata = self.opt_metadata.into_inner().flatten();

        SelectionCandidate::new(self.path, self.opt_filetype, opt_metadata, path_provenance)
    }

    pub fn filename(&self) -> &OsStr {
        self.path.file_name().unwrap_or_default()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn opt_filetype(&self) -> Option<&FileType> {
        self.opt_filetype.as_ref()
    }

    pub fn opt_metadata(&self) -> Option<&Metadata> {
        self.opt_metadata
            .get_or_init(|| {
                self.opt_dir_entry
                    .as_ref()
                    .and_then(|de| de.metadata().ok())
            })
            .as_ref()
    }

    pub fn is_entry_dir(&self, opt_path_map: Option<&RefCell<HashSet<UniqueInode>>>) -> bool {
        // must do is_dir() look up on DirEntry file_type() as look up on Path will traverse links!
        if GLOBAL_CONFIG.opt_no_traverse {
            if let Some(file_type) = self.opt_filetype() {
                return file_type.is_dir();
            }
        }

        match opt_path_map.and_then(|path_map| path_map.try_borrow_mut().ok()) {
            Some(mut locked) => {
                if dir_was_previously_listed(&self, Some(&mut locked)) {
                    return false;
                }

                return self.httm_is_dir(Some(&mut locked));
            }
            None => {
                return self.httm_is_dir(None);
            }
        }
    }

    pub fn recursive_search_filter(&self) -> bool {
        if GLOBAL_CONFIG.opt_no_filter {
            return true;
        }

        if GLOBAL_CONFIG.opt_no_hidden && self.filename().to_string_lossy().starts_with('.') {
            return false;
        }

        if GLOBAL_CONFIG.opt_one_filesystem {
            match self.opt_metadata() {
                Some(path_md) if *OPT_REQUESTED_DIR_DEV == path_md.dev() => {}
                _ => {
                    // if we can't read the metadata for a path,
                    // we probably shouldn't show it either
                    return false;
                }
            }
        }

        if let Some(file_type) = self.opt_filetype() {
            if file_type.is_dir() {
                return !self.is_path_excluded();
            }
        }

        true
    }

    fn is_path_excluded(&self) -> bool {
        // FYI path is always a relative path, but no need to canonicalize as
        // partial eq for paths is comparison of components iter
        let path = self.path();

        // never check the hidden snapshot directory for live files (duh)
        // didn't think this was possible until I saw a SMB share return
        // a .zfs dir entry
        if path.ends_with(ZFS_HIDDEN_DIRECTORY) || path.ends_with(BTRFS_SNAPPER_HIDDEN_DIRECTORY) {
            return true;
        }

        // is a common btrfs snapshot dir?
        if let Some(common_snap_dir) = OPT_COMMON_SNAP_DIR.as_deref() {
            if path == common_snap_dir {
                return true;
            }
        }

        // check whether user requested this dir specifically, then we will show
        if let Some(user_requested_dir) = GLOBAL_CONFIG.opt_requested_dir.as_ref() {
            if user_requested_dir.as_path() == path {
                return false;
            }
        }

        if cfg!(target_os = "macos") {
            return MAC_OS_HIDDEN_DIRS
                .iter()
                .any(|path_str| Path::new(path_str) == self.path());
        }

        path.is_filter_dir()
    }
}

pub trait PathDeconstruction<'a> {
    fn alias(&self) -> Option<AliasedPath<'_>>;
    fn target(&self, proximate_dataset_mount: &Path) -> Option<Box<Path>>;
    fn source(&self, opt_proximate_dataset_mount: Option<&Path>) -> Option<Box<Path>>;
    fn fs_type(&self, opt_proximate_dataset_mount: Option<&Path>) -> Option<FilesystemType>;
    fn relative_path(&'a self, proximate_dataset_mount: &'a Path) -> HttmResult<&'a Path>;
    fn proximate_dataset(&'a self) -> HttmResult<&'a Path>;
    fn live_path(&self) -> Option<Box<Path>>;
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Hash)]
pub struct PathData {
    path_buf: Box<Path>,
    opt_path_metadata: Option<PathMetadata>,
    opt_style: Option<lscolors::Style>,
    opt_file_type: Option<FileType>,
}

impl PartialEq for PathData {
    fn eq(&self, other: &Self) -> bool {
        self.path().eq(other.path())
    }
}

impl Eq for PathData {}

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
        PathData::new(path.as_ref())
    }
}

impl From<BasicDirEntryInfo> for PathData {
    fn from(basic_info: BasicDirEntryInfo) -> Self {
        let opt_metadata = basic_info
            .opt_metadata()
            .cloned()
            .or_else(|| basic_info.path().symlink_metadata().ok());

        Self::with_metadata(basic_info.path, opt_metadata)
    }
}

impl From<&SelectionCandidate> for PathData {
    fn from(selection_candidate: &SelectionCandidate) -> PathData {
        // canonicalize() on any path that DNE will throw an error
        //
        // in general we handle those cases elsewhere, like the ingest
        // of input files in Config::from for deleted relative paths, etc.
        let opt_metadata = selection_candidate
            .opt_metadata()
            .cloned()
            .or_else(|| selection_candidate.path().symlink_metadata().ok());
        let opt_path_metadata = opt_metadata.and_then(|md| PathMetadata::new(&md));
        let opt_style = selection_candidate.ls_style();

        PathData {
            path_buf: selection_candidate.path().into(),
            opt_path_metadata,
            opt_style,
            opt_file_type: selection_candidate.opt_filetype().copied(),
        }
    }
}

impl PathData {
    #[inline(always)]
    pub fn new(path: &Path) -> Self {
        let canonical_path: Box<Path> = realpath(path, RealpathFlags::ALLOW_MISSING)
            .unwrap_or_else(|_| path.to_path_buf())
            .into_boxed_path();

        let opt_metadata = std::fs::symlink_metadata(canonical_path.as_ref()).ok();

        Self::with_metadata(canonical_path, opt_metadata)
    }

    #[inline(always)]
    pub fn with_metadata(path: Box<Path>, opt_metadata: Option<Metadata>) -> Self {
        // canonicalize() on any path that DNE will throw an error
        //
        // in general we handle those cases elsewhere, like the ingest
        // of input files in Config::from for deleted relative paths, etc.
        let opt_style = opt_metadata
            .as_ref()
            .and_then(|md| ENV_LS_COLORS.style_for_path_with_metadata(&path, Some(md)))
            .copied();

        let opt_file_type = opt_metadata.as_ref().map(|md| md.file_type());

        let opt_path_metadata = opt_metadata.and_then(|md| PathMetadata::new(&md));

        Self {
            path_buf: path.into(),
            opt_path_metadata,
            opt_style,
            opt_file_type,
        }
    }

    #[inline(always)]
    pub fn without_styling(path: &Path, opt_metadata: Option<Metadata>) -> Self {
        let opt_path_metadata = opt_metadata
            .or_else(|| path.symlink_metadata().ok())
            .and_then(|md| PathMetadata::new(&md));

        Self {
            path_buf: path.into(),
            opt_path_metadata,
            opt_style: None,
            opt_file_type: None,
        }
    }

    pub fn path<'a>(&'a self) -> &'a Path {
        &self.path_buf
    }

    pub fn opt_path_metadata(&self) -> Option<PathMetadata> {
        self.opt_path_metadata
    }

    pub fn opt_style(&self) -> Option<lscolors::Style> {
        self.opt_style
            .or_else(|| ENV_LS_COLORS.style_for_path(self.path()).copied())
    }

    pub fn opt_file_type(&self) -> Option<FileType> {
        self.opt_file_type
            .or_else(|| self.path().symlink_metadata().ok().map(|md| md.file_type()))
    }

    #[inline(always)]
    pub fn metadata_infallible(&self) -> PathMetadata {
        self.opt_path_metadata
            .unwrap_or_else(|| PHANTOM_PATH_METADATA)
    }

    #[inline(always)]
    pub fn proximate_plus_neighbors(&self, proximate_dataset: &Path) -> Vec<PathBuf> {
        // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
        // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
        // we map to return the key, instead of the value
        let mut res = vec![proximate_dataset.to_path_buf()];

        match self
            .path()
            .parent()
            .map(|path| PathData::without_styling(path, None))
            .map(|path| path.proximate_dataset().ok().map(|path| path.to_owned()))
            .flatten()
            .filter(|parent_dataset| parent_dataset != proximate_dataset)
        {
            Some(parent_dataset) => {
                res.push(parent_dataset);
            }
            _ => (),
        };

        let dir_iter = std::fs::read_dir(self.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|de| match de.file_type().ok() {
                Some(ft) if ft.is_dir() => {
                    Some(PathData::without_styling(&de.path(), de.metadata().ok()))
                }
                Some(ft) if ft.is_symlink() => std::fs::read_link(de.path())
                    .ok()
                    .map(|path| PathData::without_styling(&path, None)),
                _ => None,
            })
            .filter_map(|pd| pd.proximate_dataset().ok().map(|path| path.to_owned()))
            .filter(|parent_dataset| parent_dataset != proximate_dataset);

        res.extend(dir_iter);

        res.sort();
        res.dedup();

        res
    }
}

impl<'a> PathDeconstruction<'a> for PathData {
    fn alias(&self) -> Option<AliasedPath<'_>> {
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_aliases
            .as_ref()
            .and_then(|map_of_aliases| {
                self.path().ancestors().find_map(|ancestor| {
                    map_of_aliases.get(ancestor).and_then(|metadata| {
                        Some(AliasedPath::new(
                            metadata.remote_dir(),
                            &self.path().strip_prefix(ancestor).ok()?,
                        ))
                    })
                })
            })
    }

    fn live_path(&self) -> Option<Box<Path>> {
        Some(self.path().into())
    }

    #[inline(always)]
    fn relative_path(&'a self, proximate_dataset_mount: &Path) -> HttmResult<&'a Path> {
        // path strip, if aliased
        // fallback if unable to find an alias or strip a prefix
        // (each an indication we should not be trying aliases)
        self.path()
            .strip_prefix(proximate_dataset_mount)
            .map_err(|err| err.into())
    }

    fn target(&self, proximate_dataset_mount: &Path) -> Option<Box<Path>> {
        Some(proximate_dataset_mount.into())
    }

    fn source(&self, opt_proximate_dataset_mount: Option<&Path>) -> Option<Box<Path>> {
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
        self.path()
            .ancestors()
            .skip_while(|ancestor| ancestor.components().count() > *DATASET_MAX_LEN)
            .find(|ancestor| {
                GLOBAL_CONFIG
                    .dataset_collection
                    .map_of_datasets
                    .contains_key(*ancestor)
            })
            .ok_or_else(|| {
                let description = format!(
                    "httm could not identify any proximate dataset for path: {:?}",
                    self.path()
                );
                HttmError::from(description).into()
            })
    }

    fn fs_type(&self, opt_proximate_dataset_mount: Option<&Path>) -> Option<FilesystemType> {
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
    proximate_dataset: &'a Path,
    relative_path: &'a Path,
}

impl<'a> AliasedPath<'a> {
    pub fn new(proximate_dataset: &'a Path, relative_path: &'a Path) -> Self {
        Self {
            proximate_dataset,
            relative_path,
        }
    }
    pub fn proximate_dataset(&self) -> &'a Path {
        &self.proximate_dataset
    }

    pub fn relative_path(&self) -> &'a Path {
        &self.relative_path
    }
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
    pub fn new(path_data: &'a PathData) -> Option<Self> {
        if !Self::is_zfs_snap_path(path_data) {
            return None;
        }

        Some(Self { inner: path_data })
    }

    pub fn is_zfs_snap_path(path_data: &'a PathData) -> bool {
        path_data
            .path()
            .to_string_lossy()
            .contains(ZFS_SNAPSHOT_DIRECTORY)
    }
}

impl<'a> PathDeconstruction<'a> for ZfsSnapPathGuard<'_> {
    fn alias(&self) -> Option<AliasedPath<'_>> {
        // aliases aren't allowed for snap paths
        None
    }

    fn live_path(&self) -> Option<Box<Path>> {
        self.inner
            .path()
            .to_string_lossy()
            .split_once(&format!("{ZFS_SNAPSHOT_DIRECTORY}/"))
            .and_then(|(proximate_dataset_mount, relative_and_snap_name)| {
                relative_and_snap_name
                    .split_once("/")
                    .map(|(_snap_name, relative)| {
                        PathBuf::from(proximate_dataset_mount)
                            .join(relative)
                            .into_boxed_path()
                    })
            })
    }

    fn target(&self, proximate_dataset_mount: &Path) -> Option<Box<Path>> {
        self.relative_path(proximate_dataset_mount)
            .ok()
            .map(|relative| {
                self.inner
                    .path()
                    .ancestors()
                    .zip(relative.ancestors())
                    .skip_while(|(a_path, b_path)| a_path == b_path)
                    .map(|(a_path, _b_path)| a_path)
                    .collect::<PathBuf>()
                    .into_boxed_path()
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
                let description = format!(
                    "httm could not identify any relative path for path: {:?}",
                    self.path()
                );
                HttmError::from(description).into()
            })
    }

    fn source(&self, _opt_proximate_dataset_mount: Option<&Path>) -> Option<Box<Path>> {
        let path_string = &self.inner.path().to_string_lossy();

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
                Some(PathBuf::from(res).into_boxed_path())
            }
            Some(_md) => {
                eprintln!(
                    "WARN: {:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.",
                    self.inner.path()
                );
                None
            }
            _ => {
                eprintln!(
                    "WARN: {:?} is not located on a discoverable dataset.  httm can only list snapshot names for ZFS datasets.",
                    self.inner.path()
                );
                None
            }
        }
    }

    fn proximate_dataset(&'a self) -> HttmResult<&'a Path> {
        self.inner.proximate_dataset()
    }

    fn fs_type(&self, _opt_proximate_dataset_mount: Option<&Path>) -> Option<FilesystemType> {
        Some(FilesystemType::Zfs)
    }
}

impl Serialize for PathData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PathData", 2)?;

        state.serialize_field("path", &self.path())?;
        state.serialize_field("metadata", &self.opt_path_metadata)?;
        state.end()
    }
}

impl Serialize for PathMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PathData", 2)?;

        if let PrintMode::Raw(_) = GLOBAL_CONFIG.print_mode {
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

#[derive(Copy, Clone, Debug, Hash)]
pub struct PathMetadata {
    size: u64,
    inode: u64,
    dev: u64,
    modify_time: SystemTime,
    birth_time: SystemTime,
}

impl PathMetadata {
    // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
    #[inline(always)]
    pub fn new(md: &Metadata) -> Option<Self> {
        // may fail on systems that don't collect a modify time
        let modify_time = md.modified().ok()?;
        let birth_time = md.created().ok()?;
        let inode = md.ino();
        let dev = md.dev();

        Some(PathMetadata {
            size: md.len(),
            modify_time,
            birth_time,
            inode,
            dev,
        })
    }

    // using ctime instead of mtime might be more correct as mtime can be trivially changed from user space
    // but I think we want to use mtime here? People should be able to make a snapshot "unique" with only mtime?
    #[inline(always)]
    pub fn mtime(&self) -> SystemTime {
        self.modify_time
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub fn btime(&self) -> SystemTime {
        self.birth_time
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub fn inode(&self) -> u64 {
        self.inode
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub fn dev(&self) -> u64 {
        self.dev
    }

    #[inline(always)]
    pub fn size(&self) -> u64 {
        self.size
    }
}

impl Eq for PathMetadata {}

impl PartialEq for PathMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.modify_time == other.modify_time && self.size == other.size
    }
}

impl PartialOrd for PathMetadata {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PathMetadata {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        let time_order: Ordering = self.mtime().cmp(&other.mtime());

        if time_order.is_ne() {
            return time_order;
        }

        let size_order: Ordering = self.size().cmp(&other.size());

        size_order
    }
}

pub const PHANTOM_DATE: SystemTime = SystemTime::UNIX_EPOCH;
pub const PHANTOM_SIZE: u64 = 0u64;
pub const PHANTOM_INODE: u64 = 0u64;
pub const PHANTOM_DEV: u64 = 0u64;

pub const PHANTOM_PATH_METADATA: PathMetadata = PathMetadata {
    dev: PHANTOM_DEV,
    inode: PHANTOM_INODE,
    size: PHANTOM_SIZE,
    modify_time: PHANTOM_DATE,
    birth_time: PHANTOM_DATE,
};

#[derive(Debug)]
pub struct CompareContentsContainer {
    path_data: PathData,
    hash: OnceLock<u64>,
}

impl Eq for CompareContentsContainer {}

impl PartialEq for CompareContentsContainer {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(&other).is_eq()
    }
}

impl PartialOrd for CompareContentsContainer {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CompareContentsContainer {
    #[inline(always)]
    // reverse normal ordering because comparisons should be size first here
    fn cmp(&self, other: &Self) -> Ordering {
        let size_order: Ordering = self.size().cmp(&other.size());

        if size_order.is_ne() {
            return size_order;
        }

        if matches!(GLOBAL_CONFIG.dedup_by, DedupBy::Suspect) {
            let btime_order: Ordering = self.btime().cmp(&other.btime());
            let inode_order: Ordering = self.inode().cmp(&other.inode());
            let dev_order: Ordering = self.dev().cmp(&other.dev());

            if btime_order.is_eq() && inode_order.is_eq() && dev_order.is_eq() {
                let mtime_order: Ordering = self.mtime().cmp(&other.mtime());

                if mtime_order.is_eq() {
                    return Ordering::Equal;
                }

                return mtime_order;
            }
        }

        self.cmp_file_contents(other)
    }
}

impl From<CompareContentsContainer> for PathData {
    #[inline(always)]
    fn from(value: CompareContentsContainer) -> Self {
        value.path_data
    }
}

impl From<PathData> for CompareContentsContainer {
    #[inline(always)]
    fn from(path_data: PathData) -> Self {
        Self {
            path_data,
            hash: OnceLock::new(),
        }
    }
}

impl CompareContentsContainer {
    #[allow(unused)]
    #[inline(always)]
    pub fn mtime(&self) -> SystemTime {
        self.path_data.metadata_infallible().modify_time
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn btime(&self) -> SystemTime {
        self.path_data.metadata_infallible().birth_time
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn inode(&self) -> u64 {
        self.path_data.metadata_infallible().inode
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn dev(&self) -> u64 {
        self.path_data.metadata_infallible().dev
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn size(&self) -> u64 {
        self.path_data.metadata_infallible().size
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn metadata_infallible(&self) -> PathMetadata {
        self.path_data.metadata_infallible()
    }

    #[allow(unused)]
    #[allow(unused_assignments)]
    pub fn cmp_file_contents(&self, other: &Self) -> Ordering {
        let (self_hash, other_hash): (&u64, &u64) = rayon::join(
            || {
                self.hash
                    .get_or_init(|| ChecksumFileContents::from(self.path_data.path()).checksum())
            },
            || {
                other
                    .hash
                    .get_or_init(|| ChecksumFileContents::from(other.path_data.path()).checksum())
            },
        );

        self_hash.cmp(&other_hash)
    }
}
