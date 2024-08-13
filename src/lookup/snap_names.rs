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
use crate::library::utility::find_common_path;
use crate::lookup::versions::VersionsMap;
use crate::parse::mounts::FilesystemType;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Once;

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
            .map(|(pathdata, snaps)| {
                // use par iter here because no one else is using the global rayon threadpool any more
                let snap_names: Vec<String> = snaps
                    .par_iter()
                    .filter_map(|snap_pd| {
                        let opt_proximate_dataset = pathdata.proximate_dataset().ok();

                        match pathdata.fs_type(opt_proximate_dataset) {
                            Some(FilesystemType::Zfs) => {
                                ZfsSnapPathGuard::new(snap_pd).and_then(|spd| spd.source(opt_proximate_dataset))
                            }
                            Some(FilesystemType::Btrfs(_)) => {
                                static NOTICE_NON_ZFS: std::sync::Once = Once::new();
                                
                                NOTICE_NON_ZFS.call_once( || {
                                    eprintln!("WARN: Snapshot name determination for non-ZFS datasets are merely best guess efforts.  Use with caution.");
                                });

                                if snaps.len() <= 1 {
                                    eprintln!("WARN: Could not determine snapshot name for snapshot location: {:?}", snap_pd.path());
                                    return None;
                                }

                                let opt_common_path = find_common_path(snaps.into_iter().map(|p| p.path()));

                                let Some(common_path) = opt_common_path else {
                                    eprintln!("WARN: Could not determine common path for snapshot location: {:?}", snap_pd.path());
                                    return None;
                                };

                                let path_string =  snap_pd.path().to_string_lossy();
                                let relative_path =  pathdata.relative_path(opt_proximate_dataset?).ok()?;

                                let sans_prefix = path_string.strip_prefix(common_path.to_string_lossy().as_ref())?;
                                let sans_suffix = sans_prefix.strip_suffix(relative_path.to_string_lossy().as_ref())?;
                                let trim_slashes = sans_suffix.trim_matches('/');

                                if trim_slashes.is_empty() {
                                    return None;
                                }

                                Some(PathBuf::from(trim_slashes))
                            },
                            _ => {
                                eprintln!("ERROR: LIST_SNAPS is a ZFS and btrfs only option.  Path does not appear to be on a supported dataset: {:?}", snap_pd.path());
                                None
                            }   
                        }
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
