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

use super::mounts::BaseFilesystemInfo;
use crate::filesystem::mounts::FilesystemType;
use crate::library::results::{HttmError, HttmResult};
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePathAndFsType {
    pub remote_dir: Arc<Path>,
    pub fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfAliases {
    inner: BTreeMap<Box<Path>, RemotePathAndFsType>,
}

impl From<BTreeMap<Box<Path>, RemotePathAndFsType>> for MapOfAliases {
    fn from(map: BTreeMap<Box<Path>, RemotePathAndFsType>) -> Self {
        Self { inner: map }
    }
}

impl Deref for MapOfAliases {
    type Target = BTreeMap<Box<Path>, RemotePathAndFsType>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MapOfAliases {
    pub fn new(
        base_fs_info: &mut BaseFilesystemInfo,
        opt_raw_aliases: Option<Vec<String>>,
        opt_remote_dir: Option<&String>,
        opt_local_dir: Option<&String>,
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

        let opt_snap_dir: Option<Box<Path>> = if let Some(value) = opt_remote_dir {
            Some(Box::from(Path::new(&value)))
        } else if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
            std::env::var_os("HTTM_REMOTE_DIR").map(|s| Box::from(Path::new(&s)))
        } else {
            // legacy env var name
            std::env::var_os("HTTM_SNAP_POINT").map(|s| Box::from(Path::new(&s)))
        };

        if alias_values.is_none() && opt_snap_dir.is_none() {
            return Ok(None);
        }

        let opt_local_dir: Option<Box<Path>> = if let Some(value) = opt_local_dir {
            Some(Box::from(Path::new(&value)))
        } else {
            std::env::var_os("HTTM_LOCAL_DIR").map(|s| Box::from(Path::new(&s)))
        };

        let mut aliases_iter: Vec<(Box<Path>, Box<Path>)> = match alias_values {
            Some(input_aliases) => {
                let res: Option<Vec<(Box<Path>, Box<Path>)>> = input_aliases
                    .iter()
                    .map(|alias| {
                        alias
                            .split_once(':')
                            .map(|(first, rest)| (Path::new(first).into(), Path::new(rest).into()))
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

        // user defined dir exists?: check that path contains the hidden snapshot directory
        let snap_point = opt_snap_dir.map(|snap_dir| {
            // local relative dir can be set at cmdline or as an env var,
            // but defaults to current working directory if empty
            let local_dir = opt_local_dir.unwrap_or_else(|| pwd.into());

            (snap_dir, local_dir)
        });

        if let Some(value) = snap_point {
            aliases_iter.push(value)
        }

        let map_of_aliases: BTreeMap<Box<Path>, RemotePathAndFsType> = aliases_iter
            .into_iter()
            .filter_map(|(local_dir, snap_dir)| {
                // why get snap dir?  because local dir is alias, snap dir must be a dataset
                match base_fs_info
                    .map_of_datasets
                    .get_key_value(snap_dir.as_ref())
                    .map(|(k, _v)| k.clone())
                {
                    Some(_snap_dir) if !local_dir.exists() => {
                        eprintln!(
                            "WARN: An alias path specified does not exist, or is not mounted: {:?}",
                            local_dir
                        );
                        return None;
                    }
                    Some(snap_dir) => Some((local_dir, snap_dir)),
                    None => {
                        eprintln!(
                            "WARN: An alias path specified does not exist, or is not mounted: {:?}",
                            snap_dir
                        );
                        None
                    }
                }
            })
            .filter_map(
                |(local_dir, remote_dir)| match FilesystemType::new(&remote_dir) {
                    Some(fs_type) => Some((
                        local_dir,
                        RemotePathAndFsType {
                            remote_dir,
                            fs_type,
                        },
                    )),
                    None => {
                        match base_fs_info
                            .map_of_datasets
                            .get(&remote_dir)
                            .map(|md| md.fs_type.clone())
                        {
                            Some(FilesystemType::Btrfs(opt_additional_data)) => Some((
                                local_dir,
                                RemotePathAndFsType {
                                    remote_dir,
                                    fs_type: FilesystemType::Btrfs(opt_additional_data),
                                },
                            )),
                            _ => None,
                        }
                    }
                },
            )
            .collect();

        if map_of_aliases.is_empty() {
            return Ok(None);
        }

        Ok(Some(map_of_aliases.into()))
    }
}
