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

use std::sync::Arc;

mod data {
    pub mod configure;
    pub mod path_info;
}
mod exec {
    pub mod display;
    pub mod interactive;
    pub mod recursive;
    pub mod snapshot_ops;
}
mod init {
    pub mod config;
    pub mod install_hot_keys;
}
mod library {
    pub mod utility;
}
mod lookup {
    pub mod deleted;
    pub mod file_mounts;
    pub mod versions;
}
mod parse {
    pub mod aliases;
    pub mod alts;
    pub mod mounts;
    pub mod snaps;
}

use crate::data::configure::{
    ExecMode, FilesystemType, MapOfDatasets, MapOfSnaps, MostProximateAndOptAlts,
    OptBtrfsCommonSnapDir, VecOfFilterDirs,
};
use crate::exec::display::display_mounts_for_files;
use crate::exec::interactive::interactive_exec;
use crate::exec::recursive::display_recursive_wrapper;
use crate::exec::snapshot_ops::take_snapshot;
use crate::init::config::Config;
use crate::library::utility::{print_snaps_and_live_set, HttmResult};
use crate::lookup::versions::versions_lookup_exec;

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";
pub const ROOT_DIRECTORY: &str = "/";

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
