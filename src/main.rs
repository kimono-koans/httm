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
    pub mod format;
    pub mod maps;
    pub mod num_versions;
}
mod exec {
    pub mod deleted;
    pub mod display;
    pub mod interactive;
    pub mod preview;
    pub mod prune;
    pub mod recursive;
    pub mod snapshot;
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
    pub mod prune;
    pub mod versions;
}
mod parse {
    pub mod aliases;
    pub mod alts;
    pub mod mounts;
    pub mod snaps;
}

use display::maps::ToStringMap;
use exec::prune::PruneSnapshots;
use exec::snapshot::TakeSnapshot;
use library::utility::print_output_buf;

use crate::config::generate::{Config, ExecMode};
use crate::lookup::file_mounts::MountsForFiles;

use crate::exec::display::DisplayWrapper;
use crate::exec::interactive::InteractiveBrowse;
use crate::exec::recursive::NonInteractiveRecursiveWrapper;
use crate::library::results::HttmResult;
use crate::lookup::prune::PruneMap;
use crate::lookup::versions::VersionsMap;
use crate::display::maps::format_as_map;

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
            let browse_result = InteractiveBrowse::exec(config.clone(), interactive_mode)?;
            let display_map = DisplayWrapper::new(config.as_ref(), browse_result.as_ref())?;
            let output_buf = display_map.to_string();
            print_output_buf(output_buf)
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display | ExecMode::NumVersions(_) => {
            let display_map = DisplayWrapper::new(config.as_ref(), &config.paths)?;
            let output_buf = display_map.to_string();
            print_output_buf(output_buf)
        }
        // ExecMode::NonInteractiveRecursive, ExecMode::SnapFileMount, and ExecMode::MountsForFiles will print their
        // output elsewhere
        ExecMode::NonInteractiveRecursive(_) => {
            NonInteractiveRecursiveWrapper::exec(config.clone())
        }
        ExecMode::SnapFileMount(snapshot_suffix) => {
            TakeSnapshot::exec(config.as_ref(), snapshot_suffix)
        }
        ExecMode::SnapsForFiles => {
            let prune_map: PruneMap = PruneMap::exec(config.as_ref(), &None);
            let string_map = prune_map.to_string_map();
            let output_buf = format_as_map(&string_map, &config);
            print_output_buf(output_buf)
        }
        ExecMode::Prune(restriction) => PruneSnapshots::exec(config.as_ref(), restriction),
        ExecMode::MountsForFiles => {
            let versions_map: VersionsMap = MountsForFiles::new(&config).into();
            let display_map: DisplayWrapper = DisplayWrapper::from(&config, versions_map);
            let output_buf = display_map.to_string();
            print_output_buf(output_buf)
        }
    }
}
