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
use crate::library::utility::fs_type_from_hidden_dir;
use hashbrown::HashMap;
use std::ffi::OsString;
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FilesystemType {
  Zfs,
  Btrfs,
  Nilfs2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePathAndFsType {
  pub remote_dir: PathBuf,
  pub fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfAliases {
  inner: HashMap<PathBuf, RemotePathAndFsType>,
}

impl From<HashMap<PathBuf, RemotePathAndFsType>> for MapOfAliases {
  fn from(map: HashMap<PathBuf, RemotePathAndFsType>) -> Self {
    Self { inner: map }
  }
}

impl Deref for MapOfAliases {
  type Target = HashMap<PathBuf, RemotePathAndFsType>;

  fn deref(&self) -> &Self::Target {
    &self.inner
  }
}

impl MapOfAliases {
  pub fn new(
    raw_local_dir: &Option<OsString>,
    raw_snap_dir: &Option<OsString>,
    pwd: &Path,
    opt_input_aliases: &Option<Vec<String>>,
  ) -> HttmResult<Self> {
    // user defined dir exists?: check that path contains the hidden snapshot directory
    let snap_point = raw_snap_dir.as_ref().map(|value| {
      let snap_dir = PathBuf::from(value);

      // local relative dir can be set at cmdline or as an env var,
      // but defaults to current working directory if empty
      let local_dir = match raw_local_dir {
        Some(value) => PathBuf::from(value),
        None => pwd.to_path_buf(),
      };

      (snap_dir, local_dir)
    });

    let mut aliases_iter: Vec<(PathBuf, PathBuf)> = match opt_input_aliases {
      Some(input_aliases) => {
        let res: Option<Vec<(PathBuf, PathBuf)>> = input_aliases
          .iter()
          .map(|alias| {
            alias
              .split_once(':')
              .map(|(first, rest)| (PathBuf::from(first), PathBuf::from(rest)))
          })
          .collect();

        res.ok_or_else(|| {
          HttmError::new("Must use specified delimiter (':') between aliases for MAP_ALIASES.")
        })?
      }
      None => Vec::new(),
    };

    if let Some(value) = snap_point {
      aliases_iter.push(value)
    }

    let map_of_aliases: HashMap<PathBuf, RemotePathAndFsType> = aliases_iter
      .into_iter()
      .filter_map(|(local_dir, snap_dir)| {
        if !local_dir.exists() || !snap_dir.exists() {
          [local_dir, snap_dir]
            .into_iter()
            .filter(|dir| !dir.exists())
            .for_each(|dir| {
              eprintln!(
                "Warning: An alias path specified does not exist, or is not mounted: {:?}",
                dir
              )
            });
          return None;
        }

        Some((local_dir, snap_dir))
      })
      .filter_map(|(local_dir, remote_dir)| {
        fs_type_from_hidden_dir(&remote_dir).map(|fs_type| {
          (
            local_dir,
            RemotePathAndFsType {
              remote_dir,
              fs_type,
            },
          )
        })
      })
      .collect();

    Ok(map_of_aliases.into())
  }
}
