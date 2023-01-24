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

pub struct WipeSnapshots;

impl WipeSnapshots {
    pub fn exec(config: &Config) -> HttmResult<()> {
        let snap_map: WipeMap = WipeMap::exec(config);

        if let Ok(_zfs_command) = which("zfs") {
            // Self::wipe_snaps(
            //     config,
            //     &zfs_command,
            //     &snap_map,
            // )
            println!("{:?}", snap_map);
            Ok(())
        } else {
            Err(HttmError::new(
                "'zfs' command not found. Make sure the command 'zfs' is in your path.",
            )
            .into())
        }
    }

    // fn wipe_snaps(
    //     config: &Config,
    //     zfs_command: &Path,
    //     snap_map: &WipeMap,
    // ) -> HttmResult<()> {
    //     map_snapshot_names.iter().try_for_each( |(_pool_name, snapshot_names)| {
    //         let mut process_args = vec!["snapshot".to_owned()];
    //         process_args.extend_from_slice(snapshot_names);

    //         let process_output = ExecProcess::new(zfs_command).args(&process_args).output()?;
    //         let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

    //         // stderr_string is a string not an error, so here we build an err or output
    //         if !stderr_string.is_empty() {
    //             let msg = if stderr_string.contains("cannot create snapshots : permission denied") {
    //                 "httm must have root privileges to snapshot a filesystem".to_owned()
    //             } else {
    //                 "httm was unable to take snapshots. The 'zfs' command issued the following error: ".to_owned() + stderr_string
    //             };

    //             Err(HttmError::new(&msg).into())
    //         } else {
    //             let output_buf = snapshot_names
    //                 .iter()
    //                 .map(|snap_name| {
    //                     if matches!(config.print_mode, PrintMode::RawNewline | PrintMode::RawZero)  {
    //                         let delimiter = get_delimiter(config);
    //                         format!("{}{}", &snap_name, delimiter)
    //                     } else {
    //                         format!("httm took a snapshot named: {}\n", &snap_name)
    //                     }
    //                 })
    //                 .collect();
    //             print_output_buf(output_buf)
    //         }
    //     })?;

    //     Ok(())
    // }
}
