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
use crate::parse::mounts::FilesystemType;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePathAndFsType {
    pub remote_dir: PathBuf,
    pub fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfAliases {
    inner: BTreeMap<PathBuf, RemotePathAndFsType>,
}

impl From<BTreeMap<PathBuf, RemotePathAndFsType>> for MapOfAliases {
    fn from(map: BTreeMap<PathBuf, RemotePathAndFsType>) -> Self {
        Self { inner: map }
    }
}

impl Deref for MapOfAliases {
    type Target = BTreeMap<PathBuf, RemotePathAndFsType>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MapOfAliases {
    pub fn new(
        opt_raw_aliases: Option<Vec<String>>,
        opt_remote_dir: Option<String>,
        opt_local_dir: Option<String>,
        pwd: &Path,
    ) -> HttmResult<Option<MapOfAliases>> {
        let alias_values: Option<Vec<String>> = match std::env::var_os("HTTM_MAP_ALIASES") {
            Some(env_map_alias) => Some(
                env_map_alias
                    .to_string_lossy()
                    .split_terminator(',')
                    .map(|s| s.to_owned())
                    .collect(),
            ),
            None => opt_raw_aliases,
        };

        let opt_snap_dir = if let Some(value) = opt_remote_dir {
            Some(PathBuf::from(value))
        } else if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
            std::env::var_os("HTTM_REMOTE_DIR").map(|s| PathBuf::from(s))
        } else {
            // legacy env var name
            std::env::var_os("HTTM_SNAP_POINT").map(|s| PathBuf::from(s))
        };

        if opt_snap_dir.is_some() || alias_values.is_some() {
            let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR").map(|s| PathBuf::from(s));

            let opt_local_dir = if let Some(value) = opt_local_dir {
                Some(PathBuf::from(value))
            } else {
                env_local_dir
            };

            // user defined dir exists?: check that path contains the hidden snapshot directory
            let snap_point = opt_snap_dir.map(|snap_dir| {
                // local relative dir can be set at cmdline or as an env var,
                // but defaults to current working directory if empty
                let local_dir = opt_local_dir.unwrap_or_else(|| pwd.to_path_buf());

                (snap_dir, local_dir)
            });

            let mut aliases_iter: Vec<(PathBuf, PathBuf)> = match alias_values {
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
                        HttmError::new(
                            "Must use specified delimiter (':') between aliases for MAP_ALIASES.",
                        )
                    })?
                }
                None => Vec::new(),
            };

            if let Some(value) = snap_point {
                aliases_iter.push(value)
            }

            let map_of_aliases: BTreeMap<PathBuf, RemotePathAndFsType> = aliases_iter
                .into_iter()
                .filter_map(|(local_dir, snap_dir)| {
                    if !local_dir.exists() || !snap_dir.exists() {
                        [local_dir, snap_dir]
                            .into_iter()
                            .filter(|dir| !dir.exists())
                            .for_each(|dir| {
                                eprintln!("WARN: An alias path specified does not exist, or is not mounted: {:?}", dir)
                            });
                        None
                    } else {
                        Some((local_dir, snap_dir))
                    }
                })
                .filter_map(|(local_dir, remote_dir)| {
                    FilesystemType::new(&remote_dir).map(|fs_type| {
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

            if map_of_aliases.is_empty() {
                return Ok(None);
            }

            return Ok(Some(map_of_aliases.into()));
        };

        return Ok(None);
    }
}
