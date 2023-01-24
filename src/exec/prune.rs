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

use std::path::Path;
use std::process::Command as ExecProcess;

use which::which;

use crate::config::generate::Config;
use crate::exec::interactive::{select_restore_view, ViewMode};
use crate::library::results::{HttmError, HttmResult};
use crate::lookup::prune::PruneMap;

pub struct PruneSnapshots;

impl PruneSnapshots {
    pub fn exec(config: &Config) -> HttmResult<()> {
        let prune_map: PruneMap = PruneMap::exec(config);

        if let Ok(zfs_command) = which("zfs") {
            Self::interactive_prune(config, &zfs_command, prune_map)
        } else {
            Err(HttmError::new(
                "'zfs' command not found. Make sure the command 'zfs' is in your path.",
            )
            .into())
        }
    }

    fn interactive_prune(
        config: &Config,
        zfs_command: &Path,
        prune_map: PruneMap,
    ) -> HttmResult<()> {
        let file_names_string: String = prune_map
            .keys()
            .map(|key| format!("{:?}\n", key.path_buf))
            .collect();

        let snap_names: Vec<String> = prune_map.values().flatten().cloned().collect();

        if snap_names.is_empty() {
            let msg = format!(
                "httm could not find any snapshots for the files specified: {}",
                file_names_string
            );
            return Err(HttmError::new(&msg).into());
        }

        let snap_names_string: String = snap_names
            .iter()
            .map(|value| format!("{}\n", value))
            .collect();

        let preview_buffer = format!(
            "User has requested the following file/s be pruned from snapshot/s:\n\n{}
httm will destroy the following snapshot/s:\n\n{}
Before httm destroys these snapshot/s, it would like your consent. Continue? (YES/NO)\n\
─────────────────────────────────────────────────────────────────────────────\n\
YES\n\
NO",
            file_names_string, snap_names_string
        );

        // loop until user consents or doesn't
        loop {
            let user_consent = select_restore_view(config, &preview_buffer, ViewMode::Restore)?
                .to_ascii_uppercase();

            match user_consent.as_ref() {
                "YES" | "Y" => {
                    Self::prune_snaps(config, zfs_command, &prune_map)?;

                    let result_buffer = format!(
                        "httm pruned the following file/s from a snapshot/s:\n\n{}
By destroying the following snapshot/s:\n\n{}
Prune completed successfully.",
                        file_names_string, snap_names_string
                    );

                    break eprintln!("{}", result_buffer);
                }
                "NO" | "N" => {
                    break eprintln!("User declined prune.  No files were pruned from snapshots.")
                }
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        std::process::exit(0)
    }

    fn prune_snaps(_config: &Config, zfs_command: &Path, prune_map: &PruneMap) -> HttmResult<()> {
        prune_map.iter().flat_map(|(_pathdata, snapshot_names)| snapshot_names).try_for_each( |snapshot_name| {
            let mut process_args = vec!["destroy".to_owned()];
            process_args.push(snapshot_name.to_owned());

            let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
            let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

            // stderr_string is a string not an error, so here we build an err or output
            if !stderr_string.is_empty() {
                let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                    "httm must have root privileges to destroy a filesystem".to_owned()
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
