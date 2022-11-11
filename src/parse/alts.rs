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

use crate::data::filesystem_map::{MapOfAlts, MostProximateAndOptAlts};
use crate::library::results::{HttmError, HttmResult};
use crate::MapOfDatasets;

// instead of looking up, precompute possible alt replicated mounts before exec
pub fn precompute_alt_replicated(map_of_datasets: &MapOfDatasets) -> MapOfAlts {
    map_of_datasets
        .par_iter()
        .flat_map(|(mount, _dataset_info)| {
            get_alt_replicated_datasets(mount, map_of_datasets)
                .map(|datasets| (mount.clone(), datasets))
        })
        .collect()
}

fn get_alt_replicated_datasets(
    proximate_dataset_mount: &Path,
    map_of_datasets: &MapOfDatasets,
) -> HttmResult<MostProximateAndOptAlts> {
    let proximate_dataset_fs_name = match &map_of_datasets.get(proximate_dataset_mount) {
        Some(dataset_info) => dataset_info.name.clone(),
        None => {
            return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
        }
    };

    // find a filesystem that ends with our most local filesystem name
    // but which has a prefix, like a different pool name: rpool might be
    // replicated to tank/rpool
    let mut alt_replicated_mounts: Vec<PathBuf> = map_of_datasets
        .par_iter()
        .filter(|(_mount, dataset_info)| {
            dataset_info.name != proximate_dataset_fs_name
                && dataset_info
                    .name
                    .ends_with(proximate_dataset_fs_name.as_str())
        })
        .map(|(mount, _fsname)| mount)
        .cloned()
        .collect();

    if alt_replicated_mounts.is_empty() {
        // could not find the any replicated mounts
        Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
    } else {
        alt_replicated_mounts.sort_unstable_by_key(|path| path.as_os_str().len());
        Ok(MostProximateAndOptAlts {
            proximate_dataset_mount: proximate_dataset_mount.to_path_buf(),
            opt_datasets_of_interest: Some(alt_replicated_mounts),
        })
    }
}
