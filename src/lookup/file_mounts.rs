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

use crate::data::paths::PathData;
use crate::data::paths::PathDeconstruction;
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::versions::ProximateDatasetAndOptAlts;
use crate::ExecMode;
use crate::GLOBAL_CONFIG;
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::ops::Deref;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountDisplay {
    Target,
    Source,
    RelativePath,
}

impl MountDisplay {
    pub fn display<'a, T>(&self, path: &'a T, mount: &'a PathData) -> Option<PathBuf>
    where
        T: PathDeconstruction<'a> + ?Sized,
    {
        match self {
            MountDisplay::Target => path.target(&mount.path_buf),
            MountDisplay::Source => path.source(Some(&mount.path_buf)),
            MountDisplay::RelativePath => path
                .relative_path(&mount.path_buf)
                .ok()
                .map(|path| path.to_path_buf()),
        }
    }
}

#[derive(Debug)]
pub struct MountsForFiles<'a> {
    inner: BTreeSet<ProximateDatasetAndOptAlts<'a>>,
    mount_display: &'a MountDisplay,
}

impl<'a> Deref for MountsForFiles<'a> {
    type Target = BTreeSet<ProximateDatasetAndOptAlts<'a>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> MountsForFiles<'a> {
    pub fn mount_display(&self) -> &'a MountDisplay {
        self.mount_display
    }

    pub fn new(mount_display: &'a MountDisplay) -> HttmResult<Self> {
        let is_interactive_mode = matches!(GLOBAL_CONFIG.exec_mode, ExecMode::Interactive(_));

        // we only check for phantom files in "mount for file" mode because
        // people should be able to search for deleted files in other modes
        let set: BTreeSet<ProximateDatasetAndOptAlts> = GLOBAL_CONFIG
            .paths
            .par_iter()
            .filter_map(|pd| match ProximateDatasetAndOptAlts::new(pd) {
                Ok(prox_opt_alts) => Some(prox_opt_alts),
                Err(err) => {
                    if !is_interactive_mode {
                        eprintln!("WARN: {:?}", err)
                    }
                    None
                }
            })
            .map(|prox_opt_alts| {
                if !is_interactive_mode
                    && prox_opt_alts.pathdata.metadata.is_none()
                    && prox_opt_alts.datasets_of_interest().count() == 0
                {
                    eprintln!(
                        "WARN: Input file may have never existed: {:?}",
                        prox_opt_alts.pathdata.path_buf
                    );
                }

                prox_opt_alts
            })
            .collect();

        // this is disjunctive instead of conjunctive, like the error re: versions
        // this is because I think the appropriate behavior when a path DNE is to error when requesting a mount
        // whereas re: versions, a file which DNE may still have snapshot versions
        if set
            .iter()
            .all(|prox| prox.datasets_of_interest().count() == 0)
            || set.iter().all(|prox| prox.pathdata.metadata.is_none())
        {
            return Err(HttmError::new(
                "httm could either not find any mounts for the path/s specified, or all the path do not exist, so, umm, 🤷? Please try another path.",
            )
            .into());
        }

        Ok(Self {
            inner: set,
            mount_display,
        })
    }
}
