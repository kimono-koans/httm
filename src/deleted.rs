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

use crate::lookup::{snapshot_transversal, LookupReturnType, LookupType, SearchDirs};
use crate::{Config, PathData};

use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    ffi::OsString,
    fs::{read_dir, DirEntry},
    path::Path,
};

#[allow(clippy::manual_map)]
pub fn get_deleted(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<DirEntry>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let all_deleted: Vec<DirEntry> = snapshot_transversal(
        config,
        &vec![PathData::from(requested_dir)],
        LookupType::Deleted,
    )?
    .into_par_iter()
    .map(|returned| match returned {
        LookupReturnType::Deleted(res) => Some(res),
        _ => None,
    })
    .flatten()
    .map(|boxed| *boxed)
    .collect();

    // we need to make certain that what we return from possibly multiple datasets are unique
    // as these will be the filenames that populate our interactive views, so deduplicate
    // by filename here
    let unique_deleted = if !all_deleted.is_empty() || config.opt_alt_replicated {
        let unique_deleted: HashMap<OsString, DirEntry> = all_deleted
            .into_par_iter()
            .map(|dir_entry| (dir_entry.file_name(), dir_entry))
            .collect();

        unique_deleted.into_par_iter().map(|(_, v)| v).collect()
    } else {
        all_deleted
    };

    Ok(unique_deleted)
}

pub fn get_deleted_per_dataset(
    path: &Path,
    search_dirs: &SearchDirs,
) -> Result<Vec<LookupReturnType>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get all local entries we need to compare against these to know
    // what is a deleted file
    // create a collection of local unique file names
    let unique_local_filenames: HashMap<OsString, DirEntry> = read_dir(&path)?
        .flatten()
        .par_bridge()
        .map(|dir_entry| (dir_entry.file_name(), dir_entry))
        .collect();

    // now create a collection of file names in the snap_dirs
    // create a list of unique filenames on snaps
    let unique_snap_filenames: HashMap<OsString, DirEntry> =
        read_dir(&search_dirs.hidden_snapshot_dir)?
            .flatten()
            .par_bridge()
            .map(|entry| entry.path())
            .map(|path| path.join(&search_dirs.relative_path))
            .map(|path| read_dir(&path))
            .flatten_iter()
            .flatten_iter()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), dir_entry))
            .collect();

    // compare local filenames to all unique snap filenames - none values are unique here
    let unique_deleted_versions: HashMap<OsString, DirEntry> = unique_snap_filenames
        .into_par_iter()
        .filter(|(file_name, _)| unique_local_filenames.get(file_name).is_none())
        .collect();

    let res_vec: Vec<_> = unique_deleted_versions
        .into_par_iter()
        .map(|(_, v)| Box::new(v))
        .map(LookupReturnType::Deleted)
        .collect();

    Ok(res_vec)
}
