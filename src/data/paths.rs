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
    time::{SystemTime, UNIX_EPOCH},
};

use crate::config::generate::Config;
use crate::library::results::{HttmError, HttmResult};
use crate::parse::aliases::MapOfAliases;
use crate::parse::mounts::MapOfDatasets;

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BasicDirEntryInfo {
    pub file_name: OsString,
    pub path: PathBuf,
    pub file_type: Option<FileType>,
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            file_name: dir_entry.file_name(),
            path: dir_entry.path(),
            file_type: dir_entry.file_type().ok(),
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
        // this metadata() function will not traverse symlinks
        let opt_metadata = symlink_metadata(path).ok();
        PathData::new(path, opt_metadata)
    }
}

impl From<&DirEntry> for PathData {
    fn from(dir_entry: &DirEntry) -> Self {
        // this metadata() function will not traverse symlinks
        let opt_metadata = dir_entry.metadata().ok();
        let path = dir_entry.path();
        PathData::new(&path, opt_metadata)
    }
}

impl From<&BasicDirEntryInfo> for PathData {
    fn from(basic_info: &BasicDirEntryInfo) -> Self {
        // this metadata() function will not traverse symlinks
        let opt_metadata = basic_info.path.metadata().ok();
        let path = &basic_info.path;
        PathData::new(path.as_path(), opt_metadata)
    }
}

impl PathData {
    pub fn new(path: &Path, opt_metadata: Option<Metadata>) -> Self {
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
            let time = Self::get_modify_time(&md);
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

    // using ctime instead of mtime might be more correct as mtime can be trivially changed from user space
    // but I think we want to use mtime here? People should be able to make a snapshot "unique" with only mtime?
    fn get_modify_time(md: &Metadata) -> SystemTime {
        //#[cfg(not(unix))]
        // return md.modified().unwrap_or(UNIX_EPOCH);
        //#[cfg(unix)]
        //return UNIX_EPOCH + time::Duration::new(md.ctime(), md.ctime_nsec() as i32);
        md.modified().unwrap_or(UNIX_EPOCH)
    }

    pub fn get_md_infallible(&self) -> PathMetadata {
        self.metadata.unwrap_or(PHANTOM_PATH_METADATA)
    }

    pub fn get_relative_path<'a>(
        &'a self,
        config: &Config,
        proximate_dataset_mount: &Path,
    ) -> HttmResult<&'a Path> {
        // path strip, if aliased
        if let Some(map_of_aliases) = &config.dataset_collection.opt_map_of_aliases {
            let opt_aliased_local_dir = map_of_aliases
                .iter()
                // do a search for a key with a value
                .find_map(|(local_dir, alias_info)| {
                    if alias_info.remote_dir == proximate_dataset_mount {
                        Some(local_dir)
                    } else {
                        None
                    }
                });

            // fallback if unable to find an alias or strip a prefix
            // (each an indication we should not be trying aliases)
            if let Some(local_dir) = opt_aliased_local_dir {
                if let Ok(alias_stripped_path) = self.path_buf.strip_prefix(local_dir) {
                    return Ok(alias_stripped_path);
                }
            }
        }
        // default path strip
        Ok(self.path_buf.strip_prefix(proximate_dataset_mount)?)
    }

    pub fn get_proximate_dataset<'a>(
        &'a self,
        map_of_datasets: &MapOfDatasets,
    ) -> HttmResult<&'a Path> {
        // for /usr/bin, we prefer the most proximate: /usr/bin to /usr and /
        // ancestors() iterates in this top-down order, when a value: dataset/fstype is available
        // we map to return the key, instead of the value
        self.path_buf
            .ancestors()
            .skip_while(|ancestor| ancestor.components().count() > map_of_datasets.max_len)
            .find(|ancestor| map_of_datasets.inner.contains_key(*ancestor))
            .ok_or_else(|| {
                HttmError::new(
                    "httm could not identify any qualifying dataset.  \
                    Maybe consider specifying manually at SNAP_POINT?",
                )
                .into()
            })
    }

    pub fn get_alias_dataset<'a>(&self, map_of_alias: &'a MapOfAliases) -> Option<&'a Path> {
        // find_map_first should return the first seq result with a par_iter
        // but not with a par_bridge
        self.path_buf.ancestors().find_map(|ancestor| {
            map_of_alias
                .get(ancestor)
                .map(|alias_info| alias_info.remote_dir.as_path())
        })
    }
}
