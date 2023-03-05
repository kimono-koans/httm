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
use crate::lookup::versions::MostProximateAndOptAlts;
use crate::parse::aliases::FilesystemType;
use crate::GLOBAL_CONFIG;

use super::versions::SnapDatasetType;

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
    pub fn exec(opt_filters: &Option<ListSnapsFilters>) -> Self {
        // only purge the proximate dataset
        let snaps_selected_for_search = GLOBAL_CONFIG
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let snap_name_map = Self::get_snap_names(snaps_selected_for_search, opt_filters);

        snap_name_map.deref().iter().for_each(|(pathdata, snaps)| {
            if snaps.is_empty() {
                let msg = format!(
                    "httm could not find any snapshots for the file specified: {:?}",
                    pathdata.path_buf
                );
                eprintln!("WARNING: {msg}");
            }
        });

        snap_name_map
    }

    fn get_snap_names(
        snaps_selected_for_search: &[SnapDatasetType],
        opt_filters: &Option<ListSnapsFilters>,
    ) -> SnapNameMap {
        let requested_versions = GLOBAL_CONFIG.paths.par_iter().map(|pathdata| {
            // same way we use the rayon threadpool in versions
            let snap_versions: Vec<PathData> = snaps_selected_for_search
                .iter()
                .flat_map(|dataset_type| MostProximateAndOptAlts::new(pathdata, dataset_type))
                .flat_map(|datasets_of_interest| {
                    MostProximateAndOptAlts::get_search_bundles(datasets_of_interest, pathdata)
                })
                .flatten()
                .flat_map(|search_bundle| {
                    search_bundle.get_versions_processed(&GLOBAL_CONFIG.uniqueness)
                })
                .collect();
            (pathdata.clone(), snap_versions)
        });

        let inner: BTreeMap<PathData, Vec<String>> = requested_versions
            .map(|(pathdata, vec_snaps)| {
                // use par iter here because no one else is using the global rayon threadpool any more
                let snap_names: Vec<String> = vec_snaps
                    .into_par_iter()
                    .filter_map(|pathdata| Self::snap_pathdata_to_snap_name(&pathdata))
                    .filter(|snap| {
                        if let Some(filters) = opt_filters {
                            if let Some(names) = &filters.name_filters {
                                names.iter().any(|pattern| snap.contains(pattern))
                            } else {
                                true
                            }
                        } else {
                            true
                        }
                    })
                    .collect();

                (pathdata, snap_names)
            })
            .collect();

        match opt_filters {
            Some(mode_filter) if mode_filter.omit_num_snaps != 0 => {
                let res: BTreeMap<PathData, Vec<String>> = inner
                    .into_iter()
                    .map(|(pathdata, snaps)| {
                        (
                            pathdata,
                            snaps
                                .into_iter()
                                .rev()
                                .skip(mode_filter.omit_num_snaps)
                                .rev()
                                .collect(),
                        )
                    })
                    .collect();
                res.into()
            }
            _ => inner.into(),
        }
    }

    fn snap_pathdata_to_snap_name(pathdata: &PathData) -> Option<String> {
        let path_string = &pathdata.path_buf.to_string_lossy();

        let (dataset_path, opt_snap) =
            if let Some((lhs, rhs)) = path_string.split_once(".zfs/snapshot/") {
                (Path::new(lhs), rhs.split_once('/').map(|(lhs, _rhs)| lhs))
            } else {
                return None;
            };

        let opt_dataset_md = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .inner
            .get(dataset_path);

        match opt_dataset_md {
            Some(md) if md.fs_type != FilesystemType::Zfs => {
                eprintln!("WARNING: {pathdata:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.");
                None
            }
            Some(md) => opt_snap.map(|snap| format!("{}@{snap}", md.source)),
            None => None,
        }
    }
}
