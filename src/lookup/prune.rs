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

use std::{collections::BTreeMap, io::ErrorKind, ops::Deref};

use crate::config::generate::Config;
use crate::data::paths::PathData;
use crate::lookup::versions::{MostProximateAndOptAlts, RelativePathAndSnapMounts, ONLY_PROXIMATE};
use crate::parse::aliases::FilesystemType;
use crate::display::maps::ToStringMap;

use super::file_mounts::MountsForFiles;
use super::versions::SnapDatasetType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneMap {
    inner: BTreeMap<PathData, Vec<String>>,
}

impl From<BTreeMap<PathData, Vec<String>>> for PruneMap {
    fn from(map: BTreeMap<PathData, Vec<String>>) -> Self {
        Self { inner: map }
    }
}

impl From<(PathData, Vec<String>)> for PruneMap {
    fn from(tuple: (PathData, Vec<String>)) -> Self {
        Self {
            inner: BTreeMap::from([tuple]),
        }
    }
}

impl Deref for PruneMap {
    type Target = BTreeMap<PathData, Vec<String>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl ToStringMap for PruneMap {
    fn to_string_map(&self) -> BTreeMap<String, Vec<String>> {
        self
            .iter()
            .map(|(key, value)| {
                (key.path_buf.to_string_lossy().to_string(), value.to_owned())
            })
            .collect()
    }
}

impl PruneMap {
    pub fn exec(config: &Config, opt_restriction: &Option<Vec<String>>) -> Self {
        // only prune the proximate dataset
        let snaps_selected_for_search = ONLY_PROXIMATE;

        let mount_tree = MountsForFiles::from_raw_paths(config, &config.paths);

        let dataset_names_tree: BTreeMap<PathData, String> = mount_tree
            .deref()
            .iter()
            .map(|(pathdata, mounts)| {
                let name = mounts
                    .iter()
                    .filter_map(|mount| {
                        let opt_mount_md = config
                            .dataset_collection
                            .map_of_datasets
                            .datasets
                            .get(&mount.path_buf);

                        match opt_mount_md {
                            Some(md) => {
                                if md.fs_type == FilesystemType::Zfs {
                                    Some(md.name.to_owned())
                                } else {
                                    None
                                }
                            }
                            None => None,
                        }
                    })
                    .collect();
                (pathdata.clone(), name)
            })
            .collect();

        let all_snap_names = Self::get_snap_names(
            config,
            snaps_selected_for_search,
            dataset_names_tree,
            opt_restriction,
        );

        let prune_map: PruneMap = all_snap_names.into();

        prune_map
    }

    fn get_snap_names(
        config: &Config,
        snaps_selected_for_search: &[SnapDatasetType],
        dataset_names_tree: BTreeMap<PathData, String>,
        opt_restriction: &Option<Vec<String>>,
    ) -> BTreeMap<PathData, Vec<String>> {
        let all_snap_names: BTreeMap<PathData, Vec<String>> = config
            .paths
            .iter()
            .map(|pathdata| {
                let snap_versions: Vec<PathData> = snaps_selected_for_search
                    .iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                    })
                    .flat_map(|dataset_for_search| {
                        dataset_for_search.get_search_bundles(config, pathdata)
                    })
                    .flatten()
                    .flat_map(|search_bundle| search_bundle.get_all_versions())
                    .collect();
                (pathdata, snap_versions)
            })
            .map(|(pathdata, vec_snaps)| {
                let snap_names: Vec<String> = vec_snaps
                    .into_iter()
                    .map(|pathdata| pathdata.path_buf)
                    .filter_map(|snap| {
                        snap.to_string_lossy()
                            .split_once(".zfs/snapshot/")
                            .map(|(_lhs, rhs)| rhs.to_owned())
                    })
                    .filter_map(|snap| snap.split_once('/').map(|(lhs, _rhs)| lhs.to_owned()))
                    .filter(|string| {
                        if let Some(restriction) = opt_restriction {
                            restriction.iter().any(|pat| string.contains(pat))
                        } else {
                            true
                        }
                    })
                    .collect();

                (pathdata.clone(), snap_names)
            })
            .filter_map(|(pathdata, snap_names)| {
                let opt_name = dataset_names_tree.get(&pathdata);

                let full_names: Option<Vec<String>> = opt_name.map(|dataset_name| {
                    snap_names
                        .iter()
                        .map(|snap| format!("{}@{}", dataset_name, snap))
                        .collect()
                });
                full_names.map(|names| (pathdata, names))
            })
            .collect();
        all_snap_names
    }
}

impl RelativePathAndSnapMounts {
    fn get_all_versions(&self) -> Vec<PathData> {
        // get the DirEntry for our snapshot path which will have all our possible
        // snapshots, like so: .zfs/snapshots/<some snap name>/
        //
        // BTreeMap will then remove duplicates with the same system modify time and size/file len
        let all_versions: Vec<PathData> = self
            .snap_mounts
            .iter()
            .map(|path| path.join(&self.relative_path))
            .filter_map(|joined_path| {
                match joined_path.symlink_metadata() {
                    Ok(md) => Some(PathData::new(joined_path.as_path(), Some(md))),
                    Err(err) => {
                        match err.kind() {
                            // if we do not have permissions to read the snapshot directories
                            // fail/panic printing a descriptive error instead of flattening
                            ErrorKind::PermissionDenied => {
                                eprintln!("Error: When httm tried to find a file contained within a snapshot directory, permission was denied.  \
                                Perhaps you need to use sudo or equivalent to view the contents of this snapshot (for instance, btrfs by default creates privileged snapshots).  \
                                \nDetails: {}", err);
                                std::process::exit(1)
                            },
                            // if file metadata is not found, or is otherwise not available, 
                            // continue, it simply means we do not have a snapshot of this file
                            _ => None,
                        }
                    },
                }
            })
            .collect();

        all_versions
    }
}
