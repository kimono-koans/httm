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

use std::path::Path;
use std::{collections::BTreeMap, ops::Deref};

use rayon::prelude::*;

use crate::config::generate::ListSnapsFilters;
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::VersionsMap;
use crate::parse::aliases::FilesystemType;
use crate::{GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};

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
            .into_inner()
            .into_iter()
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
                    .into_par_iter()
                    .filter_map(|pathdata| Self::deconstruct_snap_paths(&pathdata))
                    .filter(|snap| {
                        if let Some(filters) = opt_filters {
                            if let Some(names) = &filters.name_filters {
                                return names.iter().any(|pattern| snap.contains(pattern));
                            }
                        }
                        true
                    })
                    .collect();

                (pathdata, snap_names)
            })
            .map(|(pathdata, mut vec_snaps)| {
                // you *could* filter above but you wouldn't be able to return a result as easily
                if let Some(mode_filter) = opt_filters {
                    if mode_filter.omit_num_snaps != 0 {
                        let opt_amt_less = vec_snaps.len().checked_sub(mode_filter.omit_num_snaps);

                        match opt_amt_less {
                            Some(amt_less) => {
                                let _ = vec_snaps.split_off(amt_less);
                            }
                            None => {
                                return Err(HttmError::new("Number of snapshots requested to omit larger than number of snapshots.").into())
                            }
                        }
                    }
                }

                Ok((pathdata, vec_snaps))
            })
            .collect::<HttmResult<_>>()?;

        if inner.is_empty() {
            return Err(HttmError::new("All valid paths have been filtered, likely because all have no snapshots. Quitting.").into());
        }

        Ok(inner.into())
    }

    fn deconstruct_snap_paths(pathdata: &PathData) -> Option<String> {
        let path_string = &pathdata.path_buf.to_string_lossy();

        let (dataset_path, (snap, _relpath)) = if let Some((lhs, rhs)) =
            path_string.split_once(&format!("{ZFS_SNAPSHOT_DIRECTORY}/"))
        {
            (Path::new(lhs), rhs.split_once('/').unwrap_or((rhs, "")))
        } else {
            return None;
        };

        let opt_dataset_md = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(dataset_path);

        match opt_dataset_md {
            Some(md) if md.fs_type == FilesystemType::Zfs => {
                Some(format!("{}@{snap}", md.source.to_string_lossy()))
            }
            Some(_md) => {
                eprintln!("WARNING: {pathdata:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.");
                None
            }
            _ => None,
        }
    }
}
