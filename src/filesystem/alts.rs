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

use crate::filesystem::mounts::MapOfDatasets;
use crate::library::results::{HttmError, HttmResult};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOfAlts {
    inner: BTreeMap<Arc<Path>, AltMetadata>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct AltMetadata {
    pub opt_datasets_of_interest: Option<Vec<Box<Path>>>,
}

impl From<BTreeMap<Arc<Path>, AltMetadata>> for MapOfAlts {
    fn from(map: BTreeMap<Arc<Path>, AltMetadata>) -> Self {
        Self { inner: map }
    }
}

impl Deref for MapOfAlts {
    type Target = BTreeMap<Arc<Path>, AltMetadata>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MapOfAlts {
    // instead of looking up, precompute possible alt replicated mounts before exec
    pub fn new(map_of_datasets: &MapOfDatasets) -> Self {
        let inner: BTreeMap<Arc<Path>, AltMetadata> = map_of_datasets
            .par_iter()
            .flat_map(|(mount, _dataset_info)| {
                Self::from_mount(mount, map_of_datasets)
                    .ok()
                    .map(|datasets| (mount.clone(), datasets))
            })
            .collect();

        Self { inner }
    }

    fn from_mount(
        proximate_dataset_mount: &Path,
        map_of_datasets: &MapOfDatasets,
    ) -> HttmResult<AltMetadata> {
        let Some(fs_name) = map_of_datasets
            .get(proximate_dataset_mount)
            .map(|p| p.source.as_os_str())
        else {
            return Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into());
        };

        // find a filesystem that ends with our most local filesystem name
        // but which has a prefix, like a different pool name: rpool might be
        // replicated to tank/rpool
        let mut alt_replicated_mounts: Vec<Box<Path>> = map_of_datasets
            .par_iter()
            .map(|(mount, dataset_info)| (mount, &dataset_info.source))
            .filter(|(_mount, source)| source.as_os_str() != fs_name && source.ends_with(fs_name))
            .map(|(mount, _source)| mount.as_ref().into())
            .collect();

        if alt_replicated_mounts.is_empty() {
            // could not find the any replicated mounts
            Err(HttmError::new("httm was unable to detect an alternate replicated mount point.  Perhaps the replicated filesystem is not mounted?").into())
        } else {
            alt_replicated_mounts.sort_unstable_by_key(|path| path.as_os_str().len());

            Ok(AltMetadata {
                opt_datasets_of_interest: Some(alt_replicated_mounts),
            })
        }
    }
}
