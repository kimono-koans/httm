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

use crate::config::generate::{NumVersionsMode, PrintMode};
use crate::data::paths::PathData;
use crate::display_map::format::PrintAsMap;
use crate::library::utility::delimiter;
use crate::lookup::versions::VersionsMap;
use crate::{VersionsDisplayWrapper, GLOBAL_CONFIG};

impl<'a> VersionsDisplayWrapper<'a> {
    pub fn format_as_num_versions(&self, num_versions_mode: &NumVersionsMode) -> String {
        // let delimiter = get_delimiter(config);
        let delimiter = delimiter();

        let printable_map = PrintAsMap::from(&self.map);

        let map_padding = printable_map.map_padding();

        let total_num_paths = self.len();

        let print_mode = &GLOBAL_CONFIG.print_mode;

        let write_out_buffer: String = self
            .iter()
            .filter_map(|(live_version, snaps)| {
                Self::parse_num_versions(
                    num_versions_mode,
                    print_mode,
                    delimiter,
                    live_version,
                    snaps,
                    map_padding,
                    total_num_paths,
                )
            })
            .collect();

        if write_out_buffer.is_empty() {
            let msg = match num_versions_mode {
                NumVersionsMode::Multiple => {
                    "Notification: No paths which have multiple versions exist."
                }
                NumVersionsMode::SingleAll
                | NumVersionsMode::SingleNoSnap
                | NumVersionsMode::SingleWithSnap => {
                    "Notification: No paths which have only a single version exist."
                }
                // NumVersionsMode::All empty should be dealt with earlier at lookup_exec
                NumVersionsMode::AllNumerals | NumVersionsMode::AllGraph => unreachable!(),
            };
            eprintln!("{msg}");
        }

        write_out_buffer
    }

    fn parse_num_versions(
        num_versions_mode: &NumVersionsMode,
        print_mode: &PrintMode,
        delimiter: char,
        live_version: &PathData,
        snaps: &[PathData],
        padding: usize,
        total_num_paths: usize,
    ) -> Option<String> {
        let display_path = live_version.path_buf.display();

        let mut num_versions = snaps.len();

        match num_versions_mode {
            NumVersionsMode::AllGraph => {
                if !VersionsMap::is_live_version_redundant(live_version, snaps) {
                    num_versions += 1
                };

                match print_mode {
                    PrintMode::FormattedDefault => Some(format!(
                        "{:<width$} : {:*<num_versions$}{}",
                        display_path,
                        "",
                        delimiter,
                        width = padding
                    )),
                    PrintMode::FormattedNotPretty | PrintMode::RawNewline | PrintMode::RawZero => {
                        unreachable!()
                    }
                }
            }
            NumVersionsMode::AllNumerals => {
                if !VersionsMap::is_live_version_redundant(live_version, snaps) {
                    num_versions += 1
                };

                match print_mode {
                    PrintMode::FormattedDefault => Some(format!(
                        "{:<width$} : {}{}",
                        display_path,
                        num_versions,
                        delimiter,
                        width = padding
                    )),
                    PrintMode::RawNewline | PrintMode::RawZero if total_num_paths == 1 => {
                        Some(format!("{num_versions}{}", delimiter))
                    }
                    PrintMode::FormattedNotPretty | PrintMode::RawNewline | PrintMode::RawZero => {
                        Some(format!("{}\t{num_versions}{}", display_path, delimiter))
                    }
                }
            }
            NumVersionsMode::Multiple => {
                if num_versions == 0
                    || (num_versions == 1
                        && VersionsMap::is_live_version_redundant(live_version, snaps))
                {
                    None
                } else {
                    Some(format!("{display_path}{delimiter}"))
                }
            }
            NumVersionsMode::SingleAll => {
                if num_versions == 0
                    || (num_versions == 1
                        && VersionsMap::is_live_version_redundant(live_version, snaps))
                {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
            NumVersionsMode::SingleNoSnap => {
                if num_versions == 0 {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
            NumVersionsMode::SingleWithSnap => {
                if num_versions == 1 && VersionsMap::is_live_version_redundant(live_version, snaps)
                {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
        }
    }
}
