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

use std::{collections::BTreeMap, ops::Deref, path::Path, path::PathBuf};

use rayon::prelude::*;

use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::MostProximateAndOptAlts;
use crate::parse::mounts::MapOfDatasets;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfAlts {
    inner: BTreeMap<PathBuf, MostProximateAndOptAlts>,
}

impl From<BTreeMap<PathBuf, MostProximateAndOptAlts>> for MapOfAlts {
    fn from(map: BTreeMap<PathBuf, MostProximateAndOptAlts>) -> Self {
        Self { inner: map }
    }
}

impl Deref for MapOfAlts {
    type Target = BTreeMap<PathBuf, MostProximateAndOptAlts>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MapOfAlts {
    // instead of looking up, precompute possible alt replicated mounts before exec
    pub fn new(map_of_datasets: &MapOfDatasets) -> Self {
        let res: BTreeMap<PathBuf, MostProximateAndOptAlts> = map_of_datasets
            .datasets
            .par_iter()
            .flat_map(|(mount, _dataset_info)| {
                Self::get_alt_replicated_from_mount(mount, map_of_datasets)
                    .map(|datasets| (mount.clone(), datasets))
            })
            .collect();

        res.into()
    }

    fn get_alt_replicated_from_mount(
        proximate_dataset_mount: &Path,
        map_of_datasets: &MapOfDatasets,
    ) -> HttmResult<MostProximateAndOptAlts> {
        let proximate_dataset_fs_name = match &map_of_datasets.datasets.get(proximate_dataset_mount)
        {
            Some(dataset_info) => dataset_info.name.clone(),
            None => {
                return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
            }
        };

        // find a filesystem that ends with our most local filesystem name
        // but which has a prefix, like a different pool name: rpool might be
        // replicated to tank/rpool
        let mut alt_replicated_mounts: Vec<PathBuf> = map_of_datasets
            .datasets
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
}
