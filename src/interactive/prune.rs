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

use crate::config::generate::ListSnapsFilters;
use crate::interactive::view_mode::{MultiSelect, ViewMode};
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::snap_names::SnapNameMap;
use crate::lookup::versions::VersionsMap;
use crate::zfs::run_command::RunZFSCommand;

pub struct PruneSnaps;

impl PruneSnaps {
    pub fn exec(
        versions_map: VersionsMap,
        opt_filters: &Option<ListSnapsFilters>,
    ) -> HttmResult<()> {
        let snap_name_map: SnapNameMap = SnapNameMap::new(versions_map, opt_filters)?;

        let select_mode = if let Some(filters) = opt_filters {
            filters.select_mode
        } else {
            false
        };

        InteractivePrune::new(&snap_name_map, select_mode)
    }

    fn prune(snap_name_map: &SnapNameMap) -> HttmResult<()> {
        let snapshot_names: Vec<String> = snap_name_map.values().flatten().cloned().collect();

        let run_zfs = RunZFSCommand::new()?;
        run_zfs.prune(&snapshot_names)
    }
}

struct InteractivePrune;

impl InteractivePrune {
    fn new(snap_name_map: &SnapNameMap, select_mode: bool) -> HttmResult<()> {
        let file_names_string: String =
            snap_name_map.keys().fold(String::new(), |mut buffer, key| {
                buffer += format!("{:?}\n", key.path()).as_str();
                buffer
            });

        let snap_names: Vec<String> = if select_mode {
            let buffer: String = snap_name_map
                .values()
                .flatten()
                .map(|name| format!("{name}\n"))
                .collect();
            let view_mode = ViewMode::Select(None);
            view_mode.view_buffer(&buffer, MultiSelect::On)?
        } else {
            snap_name_map
                .values()
                .flatten()
                .map(|name| format!("{name}\n"))
                .collect()
        };

        let snap_names_string: String = snap_names
            .into_iter()
            .map(|name| format!("{name}\n"))
            .collect();

        let prune_buffer = format!(
            "User has requested snapshots related to the following file/s be pruned:\n\n{}\n\
            httm will destroy the following snapshot/s:\n\n{}\n\
            Before httm destroys these snapshot/s, it would like your consent. Continue? (YES/NO)\n\
            ─────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO",
            file_names_string, snap_names_string
        );

        // loop until user consents or doesn't
        loop {
            let view_mode = ViewMode::Prune;

            let selection = view_mode.view_buffer(&prune_buffer, MultiSelect::Off)?;

            let user_consent = selection
                .get(0)
                .ok_or_else(|| HttmError::new("Could not obtain the first match selected"))?;

            match user_consent.to_ascii_uppercase().as_ref() {
                "YES" | "Y" => {
                    PruneSnaps::prune(snap_name_map)?;

                    let result_buffer = format!(
                        "httm pruned snapshots related to the following file/s:\n\n{}\n\
                        By destroying the following snapshot/s:\n\n{}\n\
                        Prune completed successfully.",
                        file_names_string, snap_names_string
                    );

                    break eprintln!("{result_buffer}");
                }
                "NO" | "N" => break eprintln!("User declined prune.  No files were pruned."),
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        Ok(())
    }
}
