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

use std::ops::Deref;
use std::path::Path;

use crate::HashbrownMap;
use rayon::prelude::*;

use crate::config::generate::{Config, ListSnapsFilters, ListSnapsOfType};
use crate::data::paths::PathData;
use crate::lookup::versions::MostProximateAndOptAlts;
use crate::parse::aliases::FilesystemType;

use super::versions::SnapDatasetType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapNameMap {
    inner: HashbrownMap<PathData, Vec<String>>,
}

impl From<HashbrownMap<PathData, Vec<String>>> for SnapNameMap {
    fn from(map: HashbrownMap<PathData, Vec<String>>) -> Self {
        Self { inner: map }
    }
}

impl Deref for SnapNameMap {
    type Target = HashbrownMap<PathData, Vec<String>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SnapNameMap {
    pub fn exec(config: &Config, opt_filters: &Option<ListSnapsFilters>) -> Self {
        // only purge the proximate dataset
        let snaps_selected_for_search = config
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let snap_name_map = Self::get_snap_names(config, snaps_selected_for_search, opt_filters);

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
        config: &Config,
        snaps_selected_for_search: &[SnapDatasetType],
        opt_filters: &Option<ListSnapsFilters>,
    ) -> SnapNameMap {
        let requested_versions = config.paths.par_iter().map(|pathdata| {
            // same way we use the rayon threadpool in versions
            let snap_versions: Vec<PathData> = snaps_selected_for_search
                .iter()
                .flat_map(|dataset_type| {
                    MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                })
                .flat_map(|datasets_of_interest| {
                    MostProximateAndOptAlts::get_search_bundles(
                        config,
                        datasets_of_interest,
                        pathdata,
                    )
                })
                .flatten()
                .flat_map(|search_bundle| match opt_filters {
                    Some(mode_filters)
                        if matches!(mode_filters.type_filter, ListSnapsOfType::Unique) =>
                    {
                        search_bundle.get_unique_versions()
                    }
                    _ => search_bundle.get_all_versions(),
                })
                .collect();
            (pathdata.clone(), snap_versions)
        });

        let inner: HashbrownMap<PathData, Vec<String>> = requested_versions
            .map(|(pathdata, vec_snaps)| {
                // use par iter here because no one else is using the global rayon threadpool any more
                let snap_names: Vec<String> = vec_snaps
                    .into_par_iter()
                    .map(|pathdata| pathdata.path_buf)
                    .filter_map(|snap| {
                        snap.to_string_lossy()
                            .split_once(".zfs/snapshot/")
                            .map(|(lhs, rhs)| (lhs.to_owned(), rhs.to_owned()))
                    })
                    .filter_map(|(dataset, rest)| {
                        let opt_snap = rest.split_once('/').map(|(lhs, _rhs)| lhs);

                        let dataset_path = Path::new(&dataset);

                        let opt_dataset_md = config
                            .dataset_collection
                            .map_of_datasets
                            .inner
                            .get(dataset_path);

                        match opt_dataset_md {
                            Some(md) if md.fs_type != FilesystemType::Zfs => {
                                eprintln!("WARNING: {pathdata:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.");
                                None
                            }
                            Some(md) => {
                                opt_snap.map(|snap| format!("{}@{snap}", md.name))
                            }
                            None => None,
                        }
                    })
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
                let res: HashbrownMap<PathData, Vec<String>> = inner
                    .into_iter()
                    .map(|(pathdata, snaps)| {
                        (
                            pathdata,
                            snaps
                                .into_iter()
                                .rev()
                                .skip(mode_filter.omit_num_snaps)
                                .collect(),
                        )
                    })
                    .collect();
                res.into()
            }
            _ => inner.into(),
        }
    }
}
