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

use std::collections::BTreeMap;
use std::ops::Deref;

use rayon::prelude::*;

use crate::config::generate::MountDisplay;
use crate::data::paths::PathData;
use crate::library::iter_extensions::HttmIter;
use crate::lookup::versions::{MostProximateAndOptAlts, VersionsMap};
use crate::GLOBAL_CONFIG;

#[derive(Debug)]
pub struct MountsForFiles<'a> {
    inner: BTreeMap<PathData, Vec<PathData>>,
    mount_display: &'a MountDisplay,
}

impl<'a> From<MountsForFiles<'a>> for VersionsMap {
    fn from(map: MountsForFiles) -> Self {
        map.inner.into()
    }
}

impl<'a> Deref for MountsForFiles<'a> {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> MountsForFiles<'a> {
    pub fn get_mount_display(&self) -> &'a MountDisplay {
        self.mount_display
    }

    pub fn new(mount_display: &'a MountDisplay) -> Self {
        // we only check for phantom files in "mount for file" mode because
        // people should be able to search for deleted files in other modes
        let (non_phantom_files, phantom_files): (Vec<PathData>, Vec<PathData>) = GLOBAL_CONFIG
            .paths
            .clone()
            .into_par_iter()
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

        MountsForFiles::from_raw_paths(&non_phantom_files, mount_display)
    }

    pub fn from_raw_paths(raw_vec: &[PathData], mount_display: &'a MountDisplay) -> Self {
        let snaps_selected_for_search = GLOBAL_CONFIG
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let map: BTreeMap<PathData, Vec<PathData>> = raw_vec
            .iter()
            .map(|pathdata| {
                let datasets: Vec<MostProximateAndOptAlts> = snaps_selected_for_search
                    .iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(pathdata, dataset_type, &None)
                    })
                    .collect();
                (pathdata.clone(), datasets)
            })
            .into_group_map_by(|(pathdata, _datasets_for_search)| pathdata.clone())
            .into_iter()
            .map(|(pathdata, datasets_for_search)| {
                let datasets: Vec<PathData> = datasets_for_search
                    .into_iter()
                    .flat_map(|(_proximate_mount, datasets_for_search)| datasets_for_search)
                    .flat_map(|dataset_for_search| {
                        dataset_for_search
                            .opt_datasets_of_interest
                            .to_owned()
                            .unwrap_or_else(|| {
                                vec![dataset_for_search.proximate_dataset_mount.to_path_buf()]
                            })
                    })
                    .map(|path| PathData::from(path.as_path()))
                    .rev()
                    .collect();
                (pathdata, datasets)
            })
            .collect();

        Self {
            inner: map,
            mount_display,
        }
    }
}
