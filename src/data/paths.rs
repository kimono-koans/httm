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

use std::{
    cmp::{Ord, Ordering, PartialOrd},
    ffi::OsStr,
    fs::{symlink_metadata, DirEntry, File, FileType, Metadata},
    io::{BufRead, BufReader, ErrorKind},
    path::{Path, PathBuf},
    time::SystemTime,
};

use once_cell::sync::OnceCell;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use simd_adler32::Adler32;

use crate::parse::mounts::MapOfDatasets;
use crate::parse::mounts::MaxLen;
use crate::{config::generate::ListSnapsOfType, parse::aliases::MapOfAliases};
use crate::{
    config::generate::PrintMode,
    library::{
        results::{HttmError, HttmResult},
        utility::DateFormat,
    },
};
use crate::{
    library::utility::{date_string, display_human_size},
    GLOBAL_CONFIG,
};

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BasicDirEntryInfo {
    pub path: PathBuf,
    pub file_type: Option<FileType>,
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            path: dir_entry.path(),
            file_type: dir_entry.file_type().ok(),
        }
    }
}

impl BasicDirEntryInfo {
    pub fn filename(&self) -> &OsStr {
        self.path.file_name().unwrap_or_default()
    }
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    pub path_buf: PathBuf,
    pub metadata: Option<PathMetadata>,
}

impl PartialOrd for PathData {
    #[inline]
    fn partial_cmp(&self, other: &PathData) -> Option<Ordering> {
        Some(self.path_buf.cmp(&other.path_buf))
    }
}

impl Ord for PathData {
    #[inline]
    fn cmp(&self, other: &PathData) -> Ordering {
        self.path_buf.cmp(&other.path_buf)
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
        let path_metadata = Self::opt_metadata(opt_metadata);

        Self {
            path_buf: path,
            metadata: path_metadata,
        }
    }
}

impl PathData {
    pub fn new(path: &Path, opt_metadata: Option<Metadata>) -> Self {
        // canonicalize() on any path that DNE will throw an error
        //
        // in general we handle those cases elsewhere, like the ingest
        // of input files in Config::from for deleted relative paths, etc.
        let absolute_path: PathBuf = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        let path_metadata = Self::opt_metadata(opt_metadata);

        PathData {
            path_buf: absolute_path,
            metadata: path_metadata,
        }
    }

    // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
    fn opt_metadata(opt_metadata: Option<Metadata>) -> Option<PathMetadata> {
        opt_metadata.and_then(|md| {
            // may fail on systems that don't collect a modify time
            Self::modify_time(&md).map(|time| PathMetadata {
                size: md.len(),
                modify_time: time,
            })
        })
    }

    // using ctime instead of mtime might be more correct as mtime can be trivially changed from user space
    // but I think we want to use mtime here? People should be able to make a snapshot "unique" with only mtime?
    fn modify_time(md: &Metadata) -> Option<SystemTime> {
        //#[cfg(not(unix))]
        // return md.modified().unwrap_or(UNIX_EPOCH);
        //#[cfg(unix)]
        //return UNIX_EPOCH + time::Duration::new(md.ctime(), md.ctime_nsec() as i32);
        md.modified().ok()
    }

    #[inline]
    pub fn md_infallible(&self) -> PathMetadata {
        self.metadata.unwrap_or(PHANTOM_PATH_METADATA)
    }

    pub fn relative_path<'a>(&'a self, proximate_dataset_mount: &Path) -> HttmResult<&'a Path> {
        // path strip, if aliased
        // fallback if unable to find an alias or strip a prefix
        // (each an indication we should not be trying aliases)

        match GLOBAL_CONFIG
            .dataset_collection
            .opt_map_of_aliases
            .as_deref()
            .and_then(|map_of_aliases| {
                map_of_aliases
                    .iter()
                    // do a search for a key with a value
                    .find_map(|(local_dir, alias_info)| {
                        if alias_info.remote_dir == proximate_dataset_mount {
                            return Some(local_dir);
                        }

                        None
                    })
                    .and_then(|local_dir| self.path_buf.strip_prefix(local_dir).ok())
            }) {
            Some(alias) => Ok(alias),
            // default path strip
            None => Ok(self.path_buf.strip_prefix(proximate_dataset_mount)?),
        }
    }

    pub fn proximate_dataset<'a>(
        &'a self,
        map_of_datasets: &MapOfDatasets,
    ) -> HttmResult<&'a Path> {
        // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
        // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
        // we map to return the key, instead of the value

        let dataset_max_len = map_of_datasets.max_len();

        self.path_buf
            .ancestors()
            .skip_while(|ancestor| ancestor.components().count() > dataset_max_len)
            .find(|ancestor| map_of_datasets.contains_key(*ancestor))
            .ok_or_else(|| {
                HttmError::new(
                    "httm could not identify any qualifying dataset.  \
                    Maybe consider specifying manually at SNAP_POINT?",
                )
                .into()
            })
    }

    pub fn alias_dataset<'a>(&self, map_of_alias: &'a MapOfAliases) -> Option<&'a Path> {
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        self.path_buf.ancestors().find_map(|ancestor| {
            map_of_alias
                .get(ancestor)
                .map(|alias_info| alias_info.remote_dir.as_path())
        })
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
    pub size: u64,
    pub modify_time: SystemTime,
}

pub const PHANTOM_DATE: SystemTime = SystemTime::UNIX_EPOCH;
pub const PHANTOM_SIZE: u64 = 0u64;

pub const PHANTOM_PATH_METADATA: PathMetadata = PathMetadata {
    size: PHANTOM_SIZE,
    modify_time: PHANTOM_DATE,
};

#[derive(Eq, PartialEq)]
pub struct CompareVersionsContainer {
    pathdata: PathData,
    opt_hash: Option<OnceCell<u32>>,
}

impl From<CompareVersionsContainer> for PathData {
    fn from(container: CompareVersionsContainer) -> Self {
        container.pathdata
    }
}

impl PartialOrd for CompareVersionsContainer {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CompareVersionsContainer {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        let self_md = self.pathdata.md_infallible();
        let other_md = other.pathdata.md_infallible();

        if self_md.modify_time == other_md.modify_time {
            return self_md.size.cmp(&other_md.size);
        }

        // if files, differ re mtime, but have same size, we test by bytes whether the same
        if self_md.size == other_md.size
            && self.opt_hash.is_some()
            // if above is true/false then "&& other.opt_hash.is_some()" is the same
            && self.is_same_file(other)
        {
            return Ordering::Equal;
        }

        self_md.modify_time.cmp(&other_md.modify_time)
    }
}

impl CompareVersionsContainer {
    pub fn new(pathdata: PathData, snaps_of_type: &ListSnapsOfType) -> Self {
        let opt_hash = match snaps_of_type {
            ListSnapsOfType::UniqueContents => Some(OnceCell::new()),
            ListSnapsOfType::UniqueMetadata | ListSnapsOfType::All => None,
        };

        CompareVersionsContainer { pathdata, opt_hash }
    }

    #[inline]
    #[allow(unused_assignments)]
    fn is_same_file(&self, other: &Self) -> bool {
        // SAFETY: Unwrap will fail on opt_hash is None, here we've guarded this above
        let self_hash_cell = self
            .opt_hash
            .as_ref()
            .expect("opt_hash should be check prior to this point and must be Some");
        let other_hash_cell = other
            .opt_hash
            .as_ref()
            .expect("opt_hash should be check prior to this point and must be Some");

        let (self_hash, other_hash): (HttmResult<u32>, HttmResult<u32>) = rayon::join(
            || {
                if let Some(hash_value) = self_hash_cell.get() {
                    return Ok(*hash_value);
                }

                HashFromFile::try_from(self.pathdata.path_buf.as_path())
                    .map(|hash| *self_hash_cell.get_or_init(|| hash.into_inner()))
            },
            || {
                if let Some(hash_value) = other_hash_cell.get() {
                    return Ok(*hash_value);
                }

                HashFromFile::try_from(other.pathdata.path_buf.as_path())
                    .map(|hash| *other_hash_cell.get_or_init(|| hash.into_inner()))
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

struct HashFromFile {
    hash: u32,
}

impl HashFromFile {
    #[inline(always)]
    fn into_inner(self) -> u32 {
        self.hash
    }
}

impl TryFrom<&Path> for HashFromFile {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    #[inline(always)]
    fn try_from(path: &Path) -> HttmResult<Self> {
        const IN_BUFFER_SIZE: usize = 131_072;

        let file = File::open(path)?;

        let mut reader = BufReader::with_capacity(IN_BUFFER_SIZE, file);

        let mut hash = Adler32::default();

        loop {
            let consumed = match reader.fill_buf() {
                Ok(buf) => {
                    if buf.is_empty() {
                        let res = Self {
                            hash: hash.finish(),
                        };
                        return Ok(res);
                    }

                    hash.write(buf);
                    buf.len()
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => continue,
                    ErrorKind::UnexpectedEof => {
                        let res = Self {
                            hash: hash.finish(),
                        };
                        return Ok(res);
                    }
                    _ => return Err(err.into()),
                },
            };

            reader.consume(consumed);
        }
    }
}
