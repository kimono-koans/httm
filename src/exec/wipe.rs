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

use which::which;

use crate::config::generate::Config;

use crate::library::results::{HttmError, HttmResult};

use crate::lookup::wipe::WipeMap;

use crate::exec::interactive::select_restore_view;

pub struct WipeSnapshots;

impl WipeSnapshots {
    pub fn exec(config: &Config) -> HttmResult<()> {
        let snap_map: WipeMap = WipeMap::exec(config);

        if let Ok(zfs_command) = which("zfs") {
            Self::wipe_snaps(
                 config,
                 zfs_command.as_path(),
                 &snap_map,
             )
        } else {
            Err(HttmError::new(
                "'zfs' command not found. Make sure the command 'zfs' is in your path.",
            )
            .into())
        }
    }

    fn wipe_snaps(
        config: &Config,
        zfs_command: &Path,
        wipe_map: &WipeMap,
    ) -> HttmResult<()> {
        wipe_map.iter().try_for_each( |(pathdata, snapshot_names)| {
            let mut process_args = vec!["snapshot".to_owned()];
            process_args.extend_from_slice(snapshot_names);

            let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
            let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

            // stderr_string is a string not an error, so here we build an err or output
            if !stderr_string.is_empty() {
                let msg = if stderr_string.contains("cannot create snapshots : permission denied") {
                    "httm must have root privileges to snapshot a filesystem".to_owned()
                } else {
                    "httm was unable to take snapshots. The 'zfs' command issued the following error: ".to_owned() + stderr_string
                };

                Err(HttmError::new(&msg).into())
            } else {
                let output_buf = snapshot_names
                    .iter()
                    .map(|snap_name| {
                        if matches!(config.print_mode, PrintMode::RawNewline | PrintMode::RawZero)  {
                            let delimiter = get_delimiter(config);
                            format!("{}{}", &snap_name, delimiter)
                        } else {
                            format!("httm took a snapshot named: {}\n", &snap_name)
                        }
                    })
                    .collect();
                print_output_buf(output_buf)
            }
        })?;

        Ok(())
    }
}

struct InteractiveWipe;

impl InteractiveWipe {
    fn exec(
        config: &Config,
    ) -> HttmResult<()> {


        
        // tell the user what we're up to, and get consent
        let preview_buffer = format!(
            "httm will destroy the following snapshots:\n\n\
            \tfrom: {:?}\n\
            \tto:   {:?}\n\n\
            Before httm restores this file, it would like your consent. Continue? (YES/NO)\n\
            ──────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO",
            snap_pathdata.path_buf, new_file_path_buf
        );

        // loop until user consents or doesn't
        loop {
            let user_consent = select_restore_view(config, &preview_buffer, ViewMode::Restore)?
                .to_ascii_uppercase();

            match user_consent.as_ref() {
                "YES" | "Y" => {
                    copy_recursive(&snap_pathdata.path_buf, &new_file_path_buf, should_preserve)?;

                    let result_buffer = format!(
                        "httm copied a file from a snapshot:\n\n\
                            \tfrom: {:?}\n\
                            \tto:   {:?}\n\n\
                            Restore completed successfully.",
                        snap_pathdata.path_buf, new_file_path_buf
                    );

                    break eprintln!("{}", result_buffer);
                }
                "NO" | "N" => break eprintln!("User declined restore.  No files were restored."),
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        std::process::exit(0)
    }
