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

mod data {
    pub mod filesystem_info;
    pub mod paths;
    pub mod selection;
}
mod display {
    pub mod maps;
    pub mod num_versions;
    pub mod primary;
}
mod exec {
    pub mod interactive;
    pub mod recursive;
    pub mod snapshot;
    pub mod spawn_deleted;
}
mod config {
    pub mod generate;
    pub mod install_hot_keys;
}
mod library {
    pub mod iter_extensions;
    pub mod results;
    pub mod utility;
}
mod lookup {
    pub mod deleted;
    pub mod file_mounts;
    pub mod last_in_time;
    pub mod versions;
}
mod parse {
    pub mod aliases;
    pub mod alts;
    pub mod mounts;
    pub mod snaps;
}

use library::utility::print_output_buf;

use crate::config::generate::{Config, ExecMode};
use crate::lookup::file_mounts::MountsForFiles;

use crate::exec::interactive::interactive_exec;
use crate::exec::recursive::display_recursive_wrapper;
use crate::exec::snapshot::take_snapshot;
use crate::library::results::HttmResult;
use crate::lookup::versions::{versions_lookup_exec, DisplayMap};

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
    let config = Config::new()?;

    if config.opt_debug {
        eprintln!("{:#?}", config);
    }

    // fn exec() handles the basic display cases, and sends other cases to be processed elsewhere
    match &config.exec_mode {
        // ExecMode::Interactive *may* return back to this function to be printed
        ExecMode::Interactive(interactive_mode) => {
            let browse_result = &interactive_exec(config.clone(), interactive_mode)?;
            let display_map = versions_lookup_exec(config.as_ref(), browse_result)?;
            let output_buf = display_map.display(&config);
            print_output_buf(output_buf)
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display | ExecMode::NumVersions(_) => {
            let display_map = versions_lookup_exec(config.as_ref(), &config.paths)?;
            let output_buf = display_map.display(&config);
            print_output_buf(output_buf)
        }
        // ExecMode::DisplayRecursive, ExecMode::SnapFileMount, and ExecMode::MountsForFiles will print their
        // output elsewhere
        ExecMode::DisplayRecursive(_) => display_recursive_wrapper(config.clone()),
        ExecMode::SnapFileMount(snapshot_suffix) => take_snapshot(config.as_ref(), snapshot_suffix),
        ExecMode::MountsForFiles => {
            let display_map: DisplayMap = MountsForFiles::new(&config).into();
            let output_buf = display_map.display(&config);
            print_output_buf(output_buf)
        }
    }
}
