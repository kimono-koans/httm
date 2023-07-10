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

use std::process::Command as ExecProcess;

use crate::config::generate::ListSnapsFilters;
use crate::exec::interactive::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::snap_names::SnapNameMap;
use crate::lookup::versions::VersionsMap;

pub struct PurgeSnaps;

impl PurgeSnaps {
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

        Self::interactive_purge(&snap_name_map, select_mode)
    }

    fn interactive_purge(snap_name_map: &SnapNameMap, select_mode: bool) -> HttmResult<()> {
        let file_names_string: String = snap_name_map
            .keys()
            .map(|key| format!("{:?}\n", key.path_buf))
            .collect();

        let snap_names: Vec<String> = if select_mode {
            let buffer: String = snap_name_map
                .values()
                .flatten()
                .map(|value| format!("{value}\n"))
                .collect();
            let view_mode = &ViewMode::Select(None);
            view_mode.view(&buffer, true)?
        } else {
            snap_name_map.values().flatten().cloned().collect()
        };

        let snap_names_string: String = snap_names
            .iter()
            .map(|value| format!("{value}\n"))
            .collect();

        let preview_buffer = format!(
            "User has requested snapshots related to the following file/s be purged:\n\n{}\n\
            httm will destroy the following snapshot/s:\n\n{}\n\
            Before httm destroys these snapshot/s, it would like your consent. Continue? (YES/NO)\n\
            ─────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO",
            file_names_string, snap_names_string
        );

        // loop until user consents or doesn't
        loop {
            let view_mode = &ViewMode::Purge;
            let user_consent = view_mode.view(&preview_buffer, false)?[0].to_ascii_uppercase();

            match user_consent.as_ref() {
                "YES" | "Y" => {
                    Self::purge_snaps(snap_name_map)?;

                    let result_buffer = format!(
                        "httm purged snapshots related to the following file/s:\n\n{}\n\
                        By destroying the following snapshot/s:\n\n{}\n\
                        Purge completed successfully.",
                        file_names_string, snap_names_string
                    );

                    break eprintln!("{result_buffer}");
                }
                "NO" | "N" => break eprintln!("User declined purge.  No files were purged."),
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        std::process::exit(0)
    }

    fn purge_snaps(snap_name_map: &SnapNameMap) -> HttmResult<()> {
        let zfs_command = which::which("zfs").map_err(|_err| {
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
        })?;
        snap_name_map.values().flatten().try_for_each( |snapshot_name| {
            let process_args = vec!["destroy".to_owned(), snapshot_name.clone()];

            let process_output = ExecProcess::new(&zfs_command).args(&process_args).output()?;
            let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

            // stderr_string is a string not an error, so here we build an err or output
            if !stderr_string.is_empty() {
                let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                    "httm must have root privileges to destroy a snapshot filesystem".to_owned()
                } else {
                    "httm was unable to destroy snapshots. The 'zfs' command issued the following error: ".to_owned() + stderr_string
                };

                Err(HttmError::new(&msg).into())
            } else {
                Ok(())
            }
        })
    }
}
