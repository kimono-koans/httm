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
use crate::data::paths::PathDeconstruction;
use crate::data::paths::{PathData, ZfsSnapPathGuard};
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::VersionsMap;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;

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
            .par_iter()
            .filter(|(pathdata, snaps)| {
                if snaps.is_empty() {
                    let msg = format!(
                        "httm could not find any snapshots for the file specified: {:?}",
                        pathdata.path_buf
                    );
                    eprintln!("WARNING: {msg}");
                    return false;
                }

                true
            })
            .map(|(pathdata, vec_snaps)| {
                // use par iter here because no one else is using the global rayon threadpool any more
                let snap_names: Vec<String> = vec_snaps
                    .par_iter()
                    .filter_map(|pd| {
                        ZfsSnapPathGuard::new(pd).and_then(|spd| spd.source(None))
                    })
                    .filter(|snap| {
                        if let Some(filters) = opt_filters {
                            if let Some(names) = &filters.name_filters {
                                return names.iter().any(|pattern| snap.to_string_lossy().contains(pattern));
                            }
                        }
                        true
                    })
                    .map(|path| path.to_string_lossy().to_string())
                    .collect();

                (pathdata, snap_names)
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
