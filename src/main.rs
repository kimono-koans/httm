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
    pub mod filesystem_map;
    pub mod paths;
}
mod display {
    pub mod primary;
    pub mod special;
}
mod exec {
    pub mod interactive;
    pub mod recursive;
    pub mod snapshot;
}
mod config {
    pub mod generate;
    pub mod helper;
    pub mod install_hot_keys;
}
mod library {
    pub mod results;
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

use display::special::display_as_map;

use crate::config::generate::{Config, ExecMode};
use crate::data::filesystem_map::{
    FilesystemType, MapLiveToSnaps, MapOfDatasets, MapOfSnaps, MostProximateAndOptAlts,
    OptBtrfsCommonSnapDir, VecOfFilterDirs,
};

use crate::display::primary::display_exec;
use crate::display::special::display_mounts;
use crate::exec::interactive::interactive_exec;
use crate::exec::recursive::display_recursive_wrapper;
use crate::exec::snapshot::take_snapshot;
use crate::library::results::HttmResult;
use crate::library::utility::print_output_buf;
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
        ExecMode::Interactive(interactive_mode) => {
            let browse_result = &interactive_exec(config.clone(), interactive_mode)?;
            let map_to_live_snaps = versions_lookup_exec(config.as_ref(), browse_result)?;
            print_display_map(&config, map_to_live_snaps)?
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display | ExecMode::NumVersions(_) => {
            let map_to_live_snaps = versions_lookup_exec(config.as_ref(), &config.paths)?;
            print_display_map(&config, map_to_live_snaps)?
        }
        // ExecMode::DisplayRecursive, ExecMode::SnapFileMount, and ExecMode::MountsForFiles will print their
        // output elsewhere
        ExecMode::DisplayRecursive(_) => display_recursive_wrapper(config.clone())?,
        ExecMode::SnapFileMount(snapshot_suffix) => take_snapshot(config.clone(), snapshot_suffix)?,
        ExecMode::MountsForFiles => display_mounts(config.as_ref())?,
    }

    Ok(())
}

fn print_display_map(config: &Config, map_live_to_snaps: MapLiveToSnaps) -> HttmResult<()> {
    // why don't we just go ahead an display last snap as an exec mode?
    // because last snap is useful as a global option.  for instance, we
    // can use it in the interactive modes to skip past the select phase
    if config.opt_last_snap.is_some() && matches!(config.exec_mode, ExecMode::Display) {
        display_as_map(config, map_live_to_snaps)?
    } else {
        let output_buf = display_exec(config, map_live_to_snaps)?;
        print_output_buf(output_buf)?
    }

    Ok(())
}
