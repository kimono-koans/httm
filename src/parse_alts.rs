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

use std::{path::Path, path::PathBuf};

use rayon::prelude::*;

use crate::lookup_versions::DatasetsForSearch;
use crate::utility::HttmError;
use crate::{AHashMap as HashMap, FilesystemType};

// instead of looking up, precompute possible alt replicated mounts before exec
pub fn precompute_alt_replicated(
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> HashMap<PathBuf, Vec<PathBuf>> {
    map_of_datasets
        .par_iter()
        .flat_map(|(mount, (_dataset, _fstype))| {
            get_alt_replicated_datasets(mount, map_of_datasets)
        })
        .map(|dataset_for_search| {
            (
                dataset_for_search.proximate_dataset_mount,
                dataset_for_search.datasets_of_interest,
            )
        })
        .collect()
}

fn get_alt_replicated_datasets(
    proximate_dataset_mount: &Path,
    map_of_datasets: &HashMap<PathBuf, (String, FilesystemType)>,
) -> Result<DatasetsForSearch, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let proximate_dataset_fsname = match &map_of_datasets.get(proximate_dataset_mount) {
        Some((proximate_dataset_fsname, _)) => proximate_dataset_fsname.clone(),
        None => {
            return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
        }
    };

    // find a filesystem that ends with our most local filesystem name
    // but which has a prefix, like a different pool name: rpool might be
    // replicated to tank/rpool
    let mut alt_replicated_mounts: Vec<PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, (fs_name, _fstype))| {
            fs_name != &proximate_dataset_fsname
                && fs_name.ends_with(proximate_dataset_fsname.as_str())
        })
        .map(|(mount, _fsname)| mount)
        .cloned()
        .collect();

    if alt_replicated_mounts.is_empty() {
        // could not find the any replicated mounts
        Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
    } else {
        alt_replicated_mounts.sort_unstable_by_key(|path| path.as_os_str().len());
        Ok(DatasetsForSearch {
            proximate_dataset_mount: proximate_dataset_mount.to_path_buf(),
            datasets_of_interest: alt_replicated_mounts,
        })
    }
}
