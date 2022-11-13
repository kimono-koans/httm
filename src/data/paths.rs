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
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{
    cmp,
    ffi::OsString,
    fs::{symlink_metadata, DirEntry, FileType, Metadata},
    path::{Path, PathBuf},
    time::SystemTime,
};

use once_cell::unsync::OnceCell;

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BasicDirEntryInfo {
    pub file_name: OsString,
    pub path: PathBuf,
    pub file_type: Option<FileType>,
    pub modify_time: OnceCell<Option<SystemTime>>,
}

impl BasicDirEntryInfo {
    pub fn get_modify_time(&self) -> Option<SystemTime> {
        *self.modify_time.get_or_init(|| {
            self.path
                .symlink_metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
        })
    }
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            file_name: dir_entry.file_name(),
            path: dir_entry.path(),
            file_type: dir_entry.file_type().ok(),
            modify_time: OnceCell::new(),
        }
    }
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    pub path_buf: PathBuf,
    pub metadata: Option<PathMetadata>,
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

impl cmp::PartialOrd for PathData {
    #[inline]
    fn partial_cmp(&self, other: &PathData) -> Option<cmp::Ordering> {
        Some(self.path_buf.cmp(&other.path_buf))
    }
}

impl cmp::Ord for PathData {
    #[inline]
    fn cmp(&self, other: &PathData) -> cmp::Ordering {
        self.path_buf.cmp(&other.path_buf)
    }
}

impl From<&Path> for PathData {
    fn from(path: &Path) -> Self {
        let opt_metadata = symlink_metadata(path).ok();
        PathData::from_parts(path, opt_metadata)
    }
}

impl From<&DirEntry> for PathData {
    fn from(dir_entry: &DirEntry) -> Self {
        let opt_metadata = dir_entry.metadata().ok();
        let path = dir_entry.path();
        PathData::from_parts(&path, opt_metadata)
    }
}

impl PathData {
    pub fn from_parts(path: &Path, opt_metadata: Option<Metadata>) -> Self {
        let absolute_path: PathBuf = if path.is_relative() {
            // canonicalize() on any path that DNE will throw an error
            //
            // in general we handle those cases elsewhere, like the ingest
            // of input files in Config::from for deleted relative paths, etc.
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        };

        // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
        let metadata = opt_metadata.map(|md| {
            let len = md.len();
            // may fail on systems that don't collect a modify time
            let time = md.modified().unwrap_or(PHANTOM_DATE);
            PathMetadata {
                size: len,
                modify_time: time,
            }
        });

        PathData {
            path_buf: absolute_path,
            metadata,
        }
    }

    pub fn md_infallible(&self) -> PathMetadata {
        self.metadata.unwrap_or(PHANTOM_PATH_METADATA)
    }
}
