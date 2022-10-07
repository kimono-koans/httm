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

#[macro_use]
extern crate lazy_static;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

mod config;
mod display;
mod install_hot_keys;
mod interactive;
mod lookup_deleted;
mod lookup_file_mounts;
mod lookup_versions;
mod parse_aliases;
mod parse_alts;
mod parse_mounts;
mod parse_snaps;
mod recursive;
mod snapshot_ops;
mod utility;

use crate::config::{Config, ExecMode};
use crate::display::display_mounts_for_files;
use crate::interactive::interactive_exec;
use crate::lookup_versions::versions_lookup_exec;
use crate::parse_aliases::RemotePathAndFsType;
use crate::parse_alts::MostProximateAndOptAlts;
use crate::parse_mounts::{DatasetMetadata, FilesystemType, MountType};
use crate::recursive::display_recursive_wrapper;
use crate::snapshot_ops::take_snapshot;
use crate::utility::{print_snaps_and_live_set, PathData};

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";
pub const ROOT_DIRECTORY: &str = "/";

// wrap this complex looking error type, which is used everywhere,
// into something more simple looking. This error, FYI, is really easy to use with rayon.
pub type HttmResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub type MapOfDatasets = BTreeMap<PathBuf, DatasetMetadata>;
pub type MapOfSnaps = BTreeMap<PathBuf, VecOfSnaps>;
pub type MapOfAlts = BTreeMap<PathBuf, MostProximateAndOptAlts>;
pub type MapOfAliases = BTreeMap<PathBuf, RemotePathAndFsType>;
pub type BtrfsCommonSnapDir = PathBuf;
pub type VecOfFilterDirs = Vec<PathBuf>;
pub type VecOfSnaps = Vec<PathBuf>;
pub type SnapsAndLiveSet = [Vec<PathData>; 2];
pub type OptMapOfAlts = Option<MapOfAlts>;
pub type OptMapOfAliases = Option<MapOfAliases>;
pub type OptBtrfsCommonSnapDir = Option<BtrfsCommonSnapDir>;

fn main() {
    match exec() {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("Error: {}", error);
            std::process::exit(1)
        }
    }
}

fn exec() -> HttmResult<()> {
    // get our program args and generate a config for use
    // everywhere else
    let config = Arc::new(Config::new()?);

    if config.opt_debug {
        eprintln!("{:#?}", config);
    }

    // fn exec() handles the basic display cases, and sends other cases to be processed elsewhere
    match &config.exec_mode {
        // ExecMode::Interactive may return back to this function to be printed
        // from an interactive browse must get the paths to print to display, or continue
        // to select or restore functions
        //
        // ExecMode::LastSnap will never return back, its a shortcut to select and restore themselves
        ExecMode::Interactive(interactive_mode) => {
            let browse_result = &interactive_exec(config.clone(), interactive_mode)?;
            let snaps_and_live_set = versions_lookup_exec(config.as_ref(), browse_result)?;
            print_snaps_and_live_set(&config, &snaps_and_live_set)?
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display => {
            let snaps_and_live_set = versions_lookup_exec(config.as_ref(), &config.paths)?;
            print_snaps_and_live_set(&config, &snaps_and_live_set)?
        }
        // ExecMode::DisplayRecursive, ExecMode::SnapFileMount, and ExecMode::MountsForFiles will print their
        // output elsewhere
        ExecMode::DisplayRecursive(_) => display_recursive_wrapper(config.clone())?,
        ExecMode::SnapFileMount => take_snapshot(config.clone())?,
        ExecMode::MountsForFiles => display_mounts_for_files(config.as_ref())?,
    }

    Ok(())
}
