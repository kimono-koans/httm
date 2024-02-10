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
use std::cmp::Ordering;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum DiffType {
    Removed,
    Created,
    Modified,
    // zfs diff semantics are: old file name -> new file name
    // old file name will be the key, and new file name will be stored in the value
    Renamed(PathBuf),
}

#[derive(Debug, Clone)]
pub struct DiffEvent {
    pub path_buf: PathBuf,
    pub diff_type: DiffType,
    pub time: DiffTime,
}

impl DiffEvent {
    pub fn new(path_string: &str, diff_type: DiffType, time_str: &str) -> HttmResult<Self> {
        let path_buf = PathBuf::from(&path_string);

        Ok(Self {
            path_buf,
            diff_type,
            time: DiffTime::new(time_str)?,
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DiffTime {
    secs: u64,
    nanos: u64,
}

impl DiffTime {
    fn new(time_str: &str) -> HttmResult<Self> {
        let (secs, nanos) = time_str
            .split_once('.')
            .ok_or_else(|| HttmError::new("Could not split time string."))?;

        let time = DiffTime {
            secs: secs.parse::<u64>()?,
            nanos: nanos.parse::<u64>()?,
        };

        Ok(time)
    }
}

impl std::cmp::Ord for DiffTime {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let secs_ordering = self.secs.cmp(&other.secs);

        if secs_ordering.is_eq() {
            return self.nanos.cmp(&other.nanos);
        }

        secs_ordering
    }
}

impl std::cmp::PartialOrd for DiffTime {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
