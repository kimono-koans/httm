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

use crate::config::generate::ListSnapsFilters;
use crate::data::paths::{PathData, PathDeconstruction, ZfsSnapPathGuard};
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::VersionsMap;
use crate::parse::mounts::FilesystemType;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapNameMap {
    inner: BTreeMap<PathData, Vec<String>>,
}

impl From<BTreeMap<PathData, Vec<String>>> for SnapNameMap {
    fn from(map: BTreeMap<PathData, Vec<String>>) -> Self {
        Self { inner: map }
    }
}

impl Deref for SnapNameMap {
    type Target = BTreeMap<PathData, Vec<String>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SnapNameMap {
    pub fn new(
        versions_map: VersionsMap,
        opt_filters: &Option<ListSnapsFilters>,
    ) -> HttmResult<Self> {
        let inner: BTreeMap<PathData, Vec<String>> = versions_map
            .iter()
            .filter(|(pathdata, snaps)| {
                if snaps.is_empty() {
                    let msg = format!(
                        "httm could not find any snapshots for the file specified: {:?}",
                        pathdata.path()
                    );
                    eprintln!("WARN: {msg}");
                    return false;
                }

                true
            })
            .filter_map(|(pathdata, snaps)| {
               let opt_proximate_dataset = pathdata.proximate_dataset().ok();

               match pathdata.fs_type(opt_proximate_dataset) {
                    Some(FilesystemType::Zfs) => {
                        // use par iter here because no one else is using the global rayon threadpool any more
                        let snap_names: Vec<PathBuf> = snaps
                            .par_iter()
                            .filter_map(|snap_pd| {
                                ZfsSnapPathGuard::new(snap_pd).and_then(|spd| spd.source(opt_proximate_dataset))
                            })
                            .collect();

                        Some((pathdata, snap_names))
                    }
                    Some(FilesystemType::Btrfs(opt_additional_btrfs_data)) => {
                        if let Some(additional_btrfs_data) = opt_additional_btrfs_data {
                            if let Some(new_map) = additional_btrfs_data.snap_names.get() {
                                let values: Vec<PathBuf> = new_map.values().cloned().map(|k| k.into_path_buf()).collect();
                                return Some((pathdata, values))
                            }                             
                        }
                        
                        None
                    },
                    _ => {
                        eprintln!("ERROR: LIST_SNAPS is a ZFS and btrfs only option.  Path does not appear to be on a supported dataset: {:?}", pathdata.path());
                        None
                    }   
                }
            })
            .map(|(mount, snaps)| {
                let vec_snaps: Vec<_> = snaps.iter().map(|p| p.to_string_lossy().to_string()).collect();
                (mount, vec_snaps)
            })
            .filter(|(_pathdata, snaps)| {
                if let Some(filters) = opt_filters {
                    if let Some(names) = &filters.name_filters {
                        return names.iter().any(|pattern| snaps.iter().any(|snap| snap.contains(pattern)));
                    }
                }
                true
            })
            .filter_map(|(pathdata, mut vec_snaps)| {
                if let Some(mode_filter) = opt_filters {
                    if mode_filter.omit_num_snaps != 0 {
                        let opt_amt_less = vec_snaps.len().checked_sub(mode_filter.omit_num_snaps);

                        match opt_amt_less {
                            Some(amt_less) => {
                                let _ = vec_snaps.split_off(amt_less);
                            }
                            None => {
                                eprintln!(
                                    "Number of snapshots requested to omit larger than number of snapshots.",
                                );
                                return None
                                
                            }
                        }
                    }
                }

                Some((pathdata.to_owned(), vec_snaps))
            })
            .collect();

        if inner.is_empty() {
            return Err(
                HttmError::new(
                "All valid paths have been filtered, likely because all have no snapshots. Quitting.",
                )
                .into(),
            );
        }

        Ok(inner.into())
    }
}
