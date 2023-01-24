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

use std::{
    collections::BTreeMap,
    io::ErrorKind,
    ops::Deref,
};

use crate::config::generate::{Config};
use crate::data::paths::PathData;
use crate::lookup::versions::MostProximateAndOptAlts;
use crate::lookup::versions::RelativePathAndSnapMounts;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeMap {
    inner: BTreeMap<PathData, Vec<String>>,
}

impl From<BTreeMap<PathData, Vec<String>>> for WipeMap {
    fn from(map: BTreeMap<PathData, Vec<String>>) -> Self {
        Self { inner: map }
    }
}

impl From<(PathData, Vec<String>)> for WipeMap {
    fn from(tuple: (PathData, Vec<String>)) -> Self {
        Self {
            inner: BTreeMap::from([tuple]),
        }
    }
}

impl Deref for WipeMap {
    type Target = BTreeMap<PathData, Vec<String>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl WipeMap {
    pub fn exec(config: &Config) -> Self {
        // create vec of all local and replicated backups at once
        let snaps_selected_for_search = config
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let mount_map: BTreeMap<PathData, Vec<MostProximateAndOptAlts>> = config
            .paths
            .iter()
            .map(|pathdata| {
                let vec_search_bundles = snaps_selected_for_search
                    .iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                    })
                    .collect();
                (pathdata.clone(), vec_search_bundles)
            })
            .collect();
        
        let search_map: BTreeMap<PathData, Vec<RelativePathAndSnapMounts>> = mount_map
            .clone()
            .into_iter()
            .map(|(pathdata, vec_search_bundles)| {
                let vec_search_bundles = vec_search_bundles
                    .iter()
                    .flat_map(|dataset_for_search| {
                       dataset_for_search.get_search_bundles(config, &pathdata)
                    })
                    .flatten()
                    .collect();
                (pathdata, vec_search_bundles)
            })
            .collect();

        let raw_snap_map: BTreeMap<PathData, Vec<PathData>> = search_map
            .clone()
            .into_iter()
            .map(|(pathdata, vec_search_bundles)| {
                let converted = vec_search_bundles
                    .iter()
                    .map(|search_bundle| {
                        search_bundle.get_all_versions()
                    })
                    .flatten()
                    .collect();    
                (pathdata, converted)
            })
            .collect();

        let all_snap_versions: BTreeMap<PathData, Vec<String>> = raw_snap_map
            .clone()
            .into_iter()
            .filter_map(|(pathdata, vec_snaps)| {
                let converted = vec_snaps
                    .into_iter()
                    .map(|pathdata| pathdata.path_buf)
                    .filter_map(|snap| {
                        let snap_name = snap
                            .components()
                            .skip_while(|component| {
                                component.as_os_str() != std::path::Path::new(".zfs") || component.as_os_str() != std::path::Path::new("snapshot")
                            })
                            .next();

                        snap_name.map(|snap| snap.as_os_str().to_os_string())
                    })
                    .map(|snap| {
                        snap.to_string_lossy().into_owned()
                    })
                    .collect();
                    
                Some((pathdata, converted))
            })
            .collect();

        let wipe_map: WipeMap = all_snap_versions.into();

        wipe_map
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
