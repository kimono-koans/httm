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
use crate::lookup::versions::MostProximateAndOptAlts;
use crate::GLOBAL_CONFIG;

#[derive(Debug)]
pub struct MountsForFiles<'a> {
    inner: BTreeMap<PathData, Vec<PathData>>,
    mount_display: &'a MountDisplay,
}

impl<'a> Deref for MountsForFiles<'a> {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> MountsForFiles<'a> {
    pub fn mount_display(&self) -> &'a MountDisplay {
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
        let map: BTreeMap<PathData, Vec<PathData>> = raw_vec
            .iter()
            .flat_map(MostProximateAndOptAlts::new)
            .map(|most_prox| {
                let vec = most_prox
                    .datasets_of_interest
                    .iter()
                    .map(PathData::from)
                    .collect();
                (most_prox.pathdata.clone(), vec)
            })
            .rev()
            .collect();

        Self {
            inner: map,
            mount_display,
        }
    }
}
