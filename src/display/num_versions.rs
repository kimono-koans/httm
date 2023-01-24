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

use std::collections::BTreeMap;

use crate::config::generate::{Config, NumVersionsMode};
use crate::data::paths::PathData;
use crate::display::maps::get_map_padding;
use crate::display::maps::ToStringMap;
use crate::library::utility::get_delimiter;
use crate::lookup::versions::VersionsMap;

impl ToStringMap for VersionsMap {
    fn to_string_map(&self) -> BTreeMap<String, Vec<String>> {
        self.iter()
            .map(|(key, values)| {
                let res = values
                    .iter()
                    .map(|value| value.path_buf.to_string_lossy().to_string())
                    .collect();
                (key.path_buf.to_string_lossy().to_string(), res)
            })
            .collect()
    }
}

impl VersionsMap {
    pub fn format_as_num_versions(
        &self,
        config: &Config,
        num_versions_mode: &NumVersionsMode,
    ) -> String {
        let delimiter = get_delimiter(config);

        let write_out_buffer: String = self
            .iter()
            .filter_map(|(live_version, snaps)| {
                let map_padding = if matches!(num_versions_mode, NumVersionsMode::All) {
                    get_map_padding(&self.to_string_map())
                } else {
                    0usize
                };
                Self::parse_num_versions(
                    num_versions_mode,
                    delimiter,
                    live_version,
                    snaps,
                    map_padding,
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
                _ => unreachable!(),
            };
            eprintln!("{}", msg);
        }

        write_out_buffer
    }

    fn parse_num_versions(
        num_versions_mode: &NumVersionsMode,
        delimiter: char,
        live_version: &PathData,
        snaps: &[PathData],
        padding: usize,
    ) -> Option<String> {
        let display_path = format!("\"{}\"", live_version.path_buf.display());

        let is_live_redundant = || {
            snaps
                .iter()
                .any(|snap_version| live_version.metadata == snap_version.metadata)
        };

        match num_versions_mode {
            NumVersionsMode::All => {
                let num_versions = if is_live_redundant() {
                    snaps.len()
                } else {
                    snaps.len() + 1
                };

                if live_version.metadata.is_none() {
                    return Some(format!(
                        "{:<width$} : Path does not exist.{}",
                        display_path,
                        delimiter,
                        width = padding
                    ));
                }

                if num_versions == 1 {
                    Some(format!(
                        "{:<width$} : 1 Version available.{}",
                        display_path,
                        delimiter,
                        width = padding
                    ))
                } else {
                    Some(format!(
                        "{:<width$} : {} Version available.{}",
                        display_path,
                        num_versions,
                        delimiter,
                        width = padding
                    ))
                }
            }
            NumVersionsMode::Multiple
            | NumVersionsMode::SingleAll
            | NumVersionsMode::SingleNoSnap
            | NumVersionsMode::SingleWithSnap => {
                if live_version.metadata.is_none() {
                    return Some(format!(
                        "{} : Path does not exist.{}",
                        display_path, delimiter
                    ));
                }

                match num_versions_mode {
                    NumVersionsMode::Multiple => {
                        if snaps.is_empty() || (snaps.len() == 1 && is_live_redundant()) {
                            None
                        } else {
                            Some(format!("{}{}", display_path, delimiter))
                        }
                    }
                    NumVersionsMode::SingleAll => {
                        if snaps.is_empty() || (snaps.len() == 1 && is_live_redundant()) {
                            Some(format!("{}{}", display_path, delimiter))
                        } else {
                            None
                        }
                    }
                    NumVersionsMode::SingleNoSnap => {
                        if snaps.is_empty() {
                            Some(format!("{}{}", display_path, delimiter))
                        } else {
                            None
                        }
                    }
                    NumVersionsMode::SingleWithSnap => {
                        if !snaps.is_empty() && (snaps.len() == 1 && is_live_redundant()) {
                            Some(format!("{}{}", display_path, delimiter))
                        } else {
                            None
                        }
                    }
                    NumVersionsMode::All => unreachable!(),
                }
            }
        }
    }
}
