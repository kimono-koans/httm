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
use crate::lookup::versions::ProximateDatasetAndOptAlts;
use crate::GLOBAL_CONFIG;

#[derive(Debug)]
pub struct MountsForFiles<'a> {
    inner: BTreeMap<&'a PathData, Vec<PathData>>,
    mount_display: &'a MountDisplay,
}

impl<'a> Deref for MountsForFiles<'a> {
    type Target = BTreeMap<&'a PathData, Vec<PathData>>;

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
        let map: BTreeMap<&PathData, Vec<PathData>> = GLOBAL_CONFIG
            .paths
            .par_iter()
            .filter(|pathdata| {
                if pathdata.metadata.is_none() {
                    eprintln!("Error: Input file may not exist: {:?}", pathdata.path_buf);
                    return false;
                }

                true
            })
            .flat_map(ProximateDatasetAndOptAlts::new)
            .map(|prox_opt_alts| {
                let vec = prox_opt_alts
                    .datasets_of_interest
                    .iter()
                    .map(PathData::from)
                    .collect();
                (prox_opt_alts.pathdata, vec)
            })
            .collect();

        Self {
            inner: map,
            mount_display,
        }
    }
}
