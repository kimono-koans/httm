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
mod display_other {
    pub mod generic_maps;
    pub mod num_versions;
    pub mod wrapper;
}
mod display_versions {
    pub mod format;
    pub mod wrapper;
}
mod exec {
    pub mod deleted;
    pub mod interactive;
    pub mod preview;
    pub mod purge;
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
    pub mod snap_names;
    pub mod versions;
}
mod parse {
    pub mod aliases;
    pub mod alts;
    pub mod mounts;
    pub mod snaps;
}

use crate::display_other::generic_maps::PrintAsMap;
use exec::purge::PurgeFiles;
use exec::snapshot::TakeSnapshot;
use library::utility::print_output_buf;

use crate::config::generate::{Config, ExecMode};
use crate::lookup::file_mounts::MountsForFiles;

use crate::display_other::wrapper::OtherDisplayWrapper;
use crate::display_versions::wrapper::VersionsDisplayWrapper;
use crate::exec::interactive::InteractiveBrowse;
use crate::exec::recursive::NonInteractiveRecursiveWrapper;
use crate::library::results::HttmResult;
use crate::lookup::snap_names::SnapNameMap;
use crate::lookup::versions::VersionsMap;

type HashbrownMap<K, V> = hashbrown::HashMap<K, V>;
type HashbrownSet<K> = hashbrown::HashSet<K>;

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";
pub const ROOT_DIRECTORY: &str = "/";

fn main() {
    match exec() {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::exit(1)
        }
    }
}

fn exec() -> HttmResult<()> {
    // get our program args and generate a config for use
    // everywhere else
    let config = Config::new()?;

    if config.opt_debug {
        eprintln!("{config:#?}");
    }

    // fn exec() handles the basic display cases, and sends other cases to be processed elsewhere
    match &config.exec_mode {
        // ExecMode::Interactive *may* return back to this function to be printed
        ExecMode::Interactive(interactive_mode) => {
            let browse_result = InteractiveBrowse::exec(config.clone(), interactive_mode)?;
            let versions_map = VersionsMap::new(config.as_ref(), &browse_result)?;
            let output_buf =
                VersionsDisplayWrapper::from(config.as_ref(), versions_map).to_string();

            print_output_buf(output_buf)
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display | ExecMode::NumVersions(_) => {
            let versions_map = VersionsMap::new(config.as_ref(), &config.paths)?;
            let output_buf =
                VersionsDisplayWrapper::from(config.as_ref(), versions_map).to_string();

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
        ExecMode::SnapsForFiles(opt_filters) => {
            let snap_name_map = SnapNameMap::exec(config.as_ref(), opt_filters);
            let printable_map = PrintAsMap::from(&snap_name_map);
            let output_buf = OtherDisplayWrapper::from(config.as_ref(), printable_map).to_string();

            print_output_buf(output_buf)
        }
        ExecMode::Purge(opt_filters) => PurgeFiles::exec(config.as_ref(), opt_filters),
        ExecMode::MountsForFiles(mount_display) => {
            let mounts_map = &MountsForFiles::new(&config, mount_display);
            let printable_map: PrintAsMap = mounts_map.into();
            let output_buf = OtherDisplayWrapper::from(&config, printable_map).to_string();

            print_output_buf(output_buf)
        }
    }
}
