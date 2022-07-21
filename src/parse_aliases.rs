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

use crate::utility::{get_fs_type_from_hidden_dir, HttmError};
use crate::{AHashMap as HashMap, FilesystemType, HttmResult};

pub fn parse_aliases(
    raw_local_dir: Option<OsString>,
    raw_snap_dir: Option<OsString>,
    pwd: &Path,
    opt_input_aliases: Option<Vec<String>>,
) -> HttmResult<HashMap<PathBuf, (PathBuf, FilesystemType)>> {
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
                .map(|alias| {
                    alias
                        .split_once(':')
                        .map(|(first, rest)| (PathBuf::from(first), PathBuf::from(rest)))
                })
                .collect();

            match res.ok_or_else(|| {
                HttmError::new(
                    "Must use specified delimiter (':') between aliases for MAP_ALIASES.",
                )
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

    let map_of_aliases = aliases_iter
        .into_iter()
        .flat_map(|(local_dir, snap_dir)| {
            if local_dir.exists() && snap_dir.exists() {
                Some((local_dir, snap_dir))
            } else {
                eprintln!("Warning: At least one alias path specified does not exist, or is not mounted: {:?}:{:?}", local_dir, snap_dir);
                None
            }
        })
        .flat_map(|(local_dir, snap_dir)| {
            get_fs_type_from_hidden_dir(&snap_dir).ok().map(|fs_type| (local_dir, (snap_dir, fs_type)))
        }).collect();

    Ok(map_of_aliases)
}
