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

use crate::config::generate::MountDisplay;
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::ProximateDatasetAndOptAlts;
use crate::GLOBAL_CONFIG;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::ops::Deref;

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

    pub fn new(mount_display: &'a MountDisplay) -> HttmResult<Self> {
        // we only check for phantom files in "mount for file" mode because
        // people should be able to search for deleted files in other modes
        let map: BTreeMap<&PathData, Vec<PathData>> = GLOBAL_CONFIG
            .paths
            .par_iter()
            .filter_map(|pd| match ProximateDatasetAndOptAlts::new(pd) {
                Ok(prox_opt_alts) => Some(prox_opt_alts),
                Err(_) => {
                    eprintln!(
                        "WARN: Filesystem upon which the path resides is not supported: {:?}",
                        pd.path_buf
                    );
                    None
                }
            })
            .map(|prox_opt_alts| {
                let vec: Vec<PathData> = prox_opt_alts
                    .datasets_of_interest
                    .iter()
                    .map(PathData::from)
                    .collect();

                if prox_opt_alts.pathdata.metadata.is_none() && vec.is_empty() {
                    eprintln!(
                        "WARN: Input file may not exist: {:?}",
                        prox_opt_alts.pathdata.path_buf
                    );
                }

                (prox_opt_alts.pathdata, vec)
            })
            .collect();

        if map.values().all(std::vec::Vec::is_empty)
            && map.keys().all(|pathdata| pathdata.metadata.is_none())
        {
            return Err(HttmError::new(
                "httm could not find either any mounts for the path/s specified, so, umm, 🤷? Please try another path.",
            )
            .into());
        }

        Ok(Self {
            inner: map,
            mount_display,
        })
    }
}
