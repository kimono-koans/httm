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
    pub fn new(versions_map: VersionsMap, opt_filters: &Option<ListSnapsFilters>) -> Self {
        let snap_name_map = Self::snap_names(versions_map, opt_filters);

        snap_name_map.iter().for_each(|(pathdata, snaps)| {
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

    fn snap_names(version_map: VersionsMap, opt_filters: &Option<ListSnapsFilters>) -> SnapNameMap {
        let inner: BTreeMap<PathData, Vec<String>> = version_map
            .into_inner()
            .into_iter()
            .map(|(pathdata, vec_snaps)| {
                // use par iter here because no one else is using the global rayon threadpool any more
                let snap_names: Vec<String> = vec_snaps
                    .into_par_iter()
                    .filter_map(|pathdata| {
                        DeconstructedSnapPathData::new(&pathdata)
                            .map(|deconstructed| deconstructed.snap_name)
                    })
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
}

// allow dead code here because this could be useful re: finding ZFS objects
// zdb uses snap name and relative path for instance
#[allow(dead_code)]
pub struct DeconstructedSnapPathData {
    snap_name: String,
}

impl DeconstructedSnapPathData {
    fn new(pathdata: &PathData) -> Option<Self> {
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
            Some(md) if md.fs_type == FilesystemType::Zfs => Some(DeconstructedSnapPathData {
                snap_name: format!("{}@{snap}", md.source.to_string_lossy()),
            }),
            Some(_md) => {
                eprintln!("WARNING: {pathdata:?} is located on a non-ZFS dataset.  httm can only list snapshot names for ZFS datasets.");
                None
            }
            _ => None,
        }
    }
}
