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

use std::{ffi::OsString, path::Path, path::PathBuf};

use clap::OsValues;

use crate::utility::HttmError;
use crate::{
    AHashMap as HashMap, FilesystemType, BTRFS_SNAPPER_HIDDEN_DIRECTORY, ZFS_SNAPSHOT_DIRECTORY,
};

pub fn parse_aliases(
    raw_local_dir: Option<OsString>,
    raw_snap_dir: Option<OsString>,
    pwd: &Path,
    opt_input_aliases: Option<OsValues>,
) -> Result<
    HashMap<PathBuf, (PathBuf, FilesystemType)>,
    Box<dyn std::error::Error + Send + Sync + 'static>,
> {
    // user defined dir exists?: check that path contains the hidden snapshot directory
    let snap_point = if let Some(value) = raw_snap_dir {
        let snap_dir = PathBuf::from(value);
        // local relative dir can be set at cmdline or as an env var, but defaults to current working directory
        let local_dir = if let Some(value) = raw_local_dir {
            let local_dir: PathBuf = PathBuf::from(value);

            // little sanity check -- make sure the user defined local dir exist
            if local_dir.metadata().is_ok() {
                local_dir
            } else {
                return Err(HttmError::new(
                    "Manually set local relative directory does not exist.  Please try another.",
                )
                .into());
            }
        } else {
            pwd.to_path_buf()
        };

        Some((snap_dir, local_dir))
    } else {
        None
    };

    let mut aliases_iter: Vec<(PathBuf, PathBuf)> = match opt_input_aliases {
        Some(input_aliases) => {
            let res: Option<Vec<(PathBuf, PathBuf)>> = input_aliases
                .into_iter()
                .map(|os_str| os_str.to_string_lossy())
                .map(|os_string| {
                    os_string
                        .split_once(':')
                        .map(|(first, rest)| (PathBuf::from(first), PathBuf::from(rest)))
                })
                .collect();

            match res.ok_or_else(|| {
                HttmError::new("Must use specified delimiter (':') for MAP_ALIASES.")
            }) {
                Ok(res) => res,
                Err(err) => return Err(err.into()),
            }
        }
        None => Vec::new(),
    };

    if let Some(value) = snap_point {
        aliases_iter.push(value)
    }

    let res = aliases_iter
        .into_iter()
        .flat_map(|(local_dir, snap_dir)| {
            get_alias_fs_type(&snap_dir)
                .ok()
                .map(|fs_type| (local_dir, (snap_dir, fs_type)))
        })
        .collect();

    Ok(res)
}

fn get_alias_fs_type(
    snap_dir: &Path,
) -> Result<FilesystemType, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // set fstype, known by whether there is a ZFS hidden snapshot dir in the root dir
    let fs_type = if snap_dir.join(ZFS_SNAPSHOT_DIRECTORY).metadata().is_ok() {
        FilesystemType::Zfs
    } else if snap_dir
        .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
        .metadata()
        .is_ok()
    {
        FilesystemType::Btrfs
    } else {
        return Err(HttmError::new(
                "User defined snap point is only available for ZFS datasets and btrfs datasets snapshot-ed via snapper.",
            )
            .into());
    };

    Ok(fs_type)
}
