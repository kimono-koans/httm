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

use std::collections::BTreeMap;
use std::ops::Deref;

use itertools::Itertools;
use rayon::prelude::*;

use crate::config::generate::Config;
use crate::data::paths::PathData;
use crate::lookup::versions::{MapLiveToSnaps, MostProximateAndOptAlts};

pub struct MountsForFiles {
    inner: BTreeMap<PathData, Vec<PathData>>,
}

impl From<BTreeMap<PathData, Vec<PathData>>> for MountsForFiles {
    fn from(map: BTreeMap<PathData, Vec<PathData>>) -> Self {
        Self { inner: map }
    }
}

impl From<MountsForFiles> for BTreeMap<PathData, Vec<PathData>> {
    fn from(mounts_for_files: MountsForFiles) -> Self {
        mounts_for_files.inner
    }
}

impl From<MountsForFiles> for MapLiveToSnaps {
    fn from(map: MountsForFiles) -> Self {
        map.inner.into()
    }
}

impl Deref for MountsForFiles {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MountsForFiles {
    pub fn new(config: &Config) -> MountsForFiles {
        // we only check for phantom files in "mount for file" mode because
        // people should be able to search for deleted files in other modes
        let (non_phantom_files, phantom_files): (Vec<&PathData>, Vec<&PathData>) = config
            .paths
            .par_iter()
            .partition(|pathdata| pathdata.metadata.is_some());

        if !phantom_files.is_empty() {
            eprintln!(
                "Error: httm was unable to determine mount locations for all input files, \
            because the following files do not appear to exist: "
            );

            phantom_files
                .iter()
                .for_each(|pathdata| eprintln!("{:?}", pathdata.path_buf));
        }

        let map: BTreeMap<PathData, Vec<PathData>> = non_phantom_files
            .into_iter()
            .map(|pathdata| {
                let datasets: Vec<MostProximateAndOptAlts> = config
                    .dataset_collection
                    .snaps_selected_for_search
                    .get_value()
                    .iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                    })
                    .collect();
                (pathdata.clone(), datasets)
            })
            .into_group_map_by(|(pathdata, _snap_types_for_search)| pathdata.clone())
            .into_iter()
            .map(|(pathdata, vec_snap_types_for_search)| {
                let datasets: Vec<PathData> = vec_snap_types_for_search
                    .into_iter()
                    .flat_map(|(_proximate_mount, snap_types_for_search)| snap_types_for_search)
                    .flat_map(|snap_types_for_search| {
                        snap_types_for_search.get_datasets_of_interest()
                    })
                    .map(|path| PathData::from(path.as_path()))
                    .rev()
                    .collect();
                (pathdata, datasets)
            })
            .collect();

        MountsForFiles::from(map)
    }
}
