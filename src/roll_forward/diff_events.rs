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

use crate::library::file_ops::{Copy, Preserve, Remove};
use crate::library::results::{HttmError, HttmResult};
use crate::roll_forward::exec::RollForward;
use nu_ansi_term::Color::{Blue, Red};
use std::cmp::Ordering;
use std::path::Path;
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

#[derive(Debug, Clone)]
pub struct DiffEvent {
    pub path_buf: PathBuf,
    pub diff_type: DiffType,
    pub time: DiffTime,
}

impl DiffEvent {
    pub fn new(line: &str) -> HttmResult<DiffEvent> {
        let split_line: Vec<&str> = line.split('\t').collect();

        let time_str = split_line
            .first()
            .ok_or_else(|| HttmError::new("Could not obtain a timestamp for diff event."))?;

        let diff_type = split_line.get(1);

        let path = split_line
            .get(2)
            .ok_or_else(|| HttmError::new("Could not obtain a path for diff event."))?;

        match diff_type {
            Some(&"-") => DiffEvent::from_parts(path, DiffType::Removed, time_str),
            Some(&"+") => DiffEvent::from_parts(path, DiffType::Created, time_str),
            Some(&"M") => DiffEvent::from_parts(path, DiffType::Modified, time_str),
            Some(&"R") => {
                let new_file_name = split_line.get(3).ok_or_else(|| {
                    HttmError::new("Could not obtain a new file name for diff event.")
                })?;

                DiffEvent::from_parts(
                    path,
                    DiffType::Renamed(PathBuf::from(new_file_name)),
                    time_str,
                )
            }
            _ => HttmError::new("Could not parse diff event").into(),
        }
    }

    fn from_parts(path_string: &str, diff_type: DiffType, time_str: &str) -> HttmResult<Self> {
        let path_buf = PathBuf::from(&path_string);

        Ok(Self {
            path_buf,
            diff_type,
            time: DiffTime::new(time_str)?,
        })
    }

    pub fn reverse_action(&self, roll_forward: &RollForward) -> HttmResult<()> {
        let live_file_path = self.path_buf.as_ref();
        let snap_file_path = roll_forward
            .snap_path(&live_file_path)
            .ok_or_else(|| HttmError::new("Could not obtain snap file path for live version."))?;

        // zfs-diff can return multiple file actions for a single inode
        // since we exclude older file actions, if rename or created is the last action,
        // we should make sure it has the latest data, so a simple rename is not enough
        // this is internal to the fn Self::remove()
        match &self.diff_type {
            DiffType::Created | DiffType::Removed | DiffType::Modified => {
                Self::overwrite_or_remove(&snap_file_path, live_file_path)
            }
            DiffType::Renamed(new_file_name) => {
                Self::overwrite_or_remove(&snap_file_path, new_file_name)?;

                Ok(())
            }
        }
    }

    pub fn copy(src: &Path, dst: &Path) -> HttmResult<()> {
        if let Err(err) = Copy::direct_quiet(src, dst, true) {
            eprintln!("Error: {}", err);
            let description = format!(
                "Could not overwrite {:?} with snapshot file version {:?}",
                dst, src
            );
            return HttmError::from(description).into();
        }

        Preserve::direct(src, dst)?;

        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);
        Ok(())
    }

    fn overwrite_or_remove(src: &Path, dst: &Path) -> HttmResult<()> {
        // overwrite
        if src.exists() {
            return Self::copy(src, dst);
        }

        // or remove
        Self::remove(dst)
    }

    pub fn remove(dst: &Path) -> HttmResult<()> {
        // overwrite
        if !dst.exists() {
            return Ok(());
        }

        match Remove::recursive_quiet(dst) {
            Ok(_) => {
                if dst.exists() {
                    let description = format!("File should not exist after deletion {:?}", dst);
                    return HttmError::from(description).into();
                }
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                let description = format!("Could not delete file {:?}", dst);
                return HttmError::from(description).into();
            }
        }

        eprintln!("{}: {:?} -> 🗑️", Red.paint("Removed  "), dst);

        Ok(())
    }
}
