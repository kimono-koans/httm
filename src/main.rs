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

mod data {
    pub mod filesystem_info;
    pub mod paths;
    pub mod selection;
}
mod display_map {
    pub mod format;
}
mod display_versions {
    pub mod format;
    pub mod num_versions;
    pub mod wrapper;
}
mod background {
    pub mod deleted;
    pub mod recursive;
}
mod interactive {
    pub mod browse;
    pub mod preview;
    pub mod prune;
    pub mod restore;
    pub mod select;
    pub mod view_mode;
}
mod roll_forward {
    pub mod diff_events;
    pub mod exec;
    pub mod preserve_hard_links;
}
mod config {
    pub mod generate;
    pub mod install_hot_keys;
}
mod library {
    pub mod diff_copy;
    pub mod file_ops;
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
mod zfs {
    pub mod run_command;
    pub mod snap_guard;
    pub mod snap_mounts;
}

use crate::config::generate::InteractiveMode;
use crate::interactive::browse::InteractiveBrowse;
use crate::interactive::select::InteractiveSelect;
use background::recursive::NonInteractiveRecursiveWrapper;
use config::generate::{Config, ExecMode};
use display_map::format::PrintAsMap;
use display_versions::wrapper::VersionsDisplayWrapper;
use interactive::prune::PruneSnaps;
use interactive::restore::InteractiveRestore;
use library::results::HttmResult;
use library::utility::print_output_buf;
use lookup::file_mounts::MountsForFiles;
use lookup::snap_names::SnapNameMap;
use lookup::versions::VersionsMap;
use roll_forward::exec::RollForward;
use std::sync::LazyLock;
use zfs::snap_mounts::SnapshotMounts;

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const TM_DIR_REMOTE: &str = "/Volumes/.timemachine";
pub const TM_DIR_LOCAL: &str = "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";
pub const NILFS2_SNAPSHOT_ID_KEY: &str = "cp=";
pub const RESTIC_SNAPSHOT_DIRECTORY: &str = "snapshots";
pub const RESTIC_LATEST_SNAPSHOT_DIRECTORY: &str = "snapshots/latest";
pub const IN_BUFFER_SIZE: usize = 131_072;

fn main() {
    match exec() {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("ERROR: {error}");
            std::process::exit(1)
        }
    }
}

// get our program args and generate a config for use
// everywhere else
static GLOBAL_CONFIG: LazyLock<Config> = LazyLock::new(|| {
    Config::new()
        .map_err(|error| {
            eprintln!("Error: {error}");
            std::process::exit(1)
        })
        .unwrap()
});

fn exec() -> HttmResult<()> {
    // fn exec() handles the basic display cases, and sends other cases to be processed elsewhere
    match &GLOBAL_CONFIG.exec_mode {
        // ExecMode::Interactive *may* return back to this function to be printed
        ExecMode::Interactive(interactive_mode) => {
            let mut browse_result = InteractiveBrowse::new()?;

            match interactive_mode {
                InteractiveMode::Restore(_) => {
                    let interactive_select = InteractiveSelect::try_from(&mut browse_result)?;

                    let interactive_restore = InteractiveRestore::from(interactive_select);

                    interactive_restore.restore()
                }
                InteractiveMode::Select(select_mode) => {
                    let interactive_select = InteractiveSelect::try_from(&mut browse_result)?;

                    interactive_select.print_selections(&select_mode)
                }
                // InteractiveMode::Browse executes back through fn exec() in main.rs
                InteractiveMode::Browse => {
                    let versions_map =
                        VersionsMap::new(&GLOBAL_CONFIG, &browse_result.selected_pathdata)?;

                    let output_buf =
                        VersionsDisplayWrapper::from(&GLOBAL_CONFIG, versions_map).to_string();

                    print_output_buf(&output_buf)
                }
            }
        }
        // ExecMode::BasicDisplay will be just printed, we already know the paths
        ExecMode::BasicDisplay | ExecMode::NumVersions(_) => {
            let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &GLOBAL_CONFIG.paths)?;
            let output_buf = VersionsDisplayWrapper::from(&GLOBAL_CONFIG, versions_map).to_string();

            print_output_buf(&output_buf)
        }
        // ExecMode::NonInteractiveRecursive, ExecMode::SnapFileMount, and ExecMode::MountsForFiles will print their
        // output elsewhere
        ExecMode::NonInteractiveRecursive(_) => NonInteractiveRecursiveWrapper::exec(),
        ExecMode::SnapFileMount(snapshot_suffix) => SnapshotMounts::exec(snapshot_suffix),
        ExecMode::SnapsForFiles(opt_filters) => {
            let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &GLOBAL_CONFIG.paths)?;
            let snap_name_map = SnapNameMap::new(versions_map, opt_filters)?;
            let printable_map = PrintAsMap::from(&snap_name_map);
            let output_buf = printable_map.to_string();

            print_output_buf(&output_buf)
        }
        ExecMode::Prune(opt_filters) => {
            let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &GLOBAL_CONFIG.paths)?;
            PruneSnaps::exec(versions_map, opt_filters)
        }
        ExecMode::MountsForFiles(mount_display) => {
            let mounts_map = &MountsForFiles::new(mount_display)?;
            let printable_map: PrintAsMap = mounts_map.into();
            let output_buf = printable_map.to_string();

            print_output_buf(&output_buf)
        }
        ExecMode::RollForward(full_snap_name) => RollForward::new(full_snap_name)?.exec(),
    }
}
