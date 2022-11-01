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
use crate::data::filesystem_map::MapLiveToSnaps;
use crate::data::paths::PathData;
use crate::display::primary::{
    display_raw, NOT_SO_PRETTY_FIXED_WIDTH_PADDING, QUOTATION_MARKS_LEN,
};
use crate::library::results::HttmResult;
use crate::library::utility::{get_delimiter, print_output_buf};
use crate::lookup::file_mounts::get_mounts_for_files;

pub fn display_num_versions(
    config: &Config,
    num_versions_mode: &NumVersionsMode,
    map_live_to_snaps: &MapLiveToSnaps,
) -> HttmResult<String> {
    let delimiter = get_delimiter(config);

    let write_out_buffer: String = map_live_to_snaps
        .iter()
        .filter_map(|(live_version, snaps)| {
            let map_padding = if matches!(num_versions_mode, NumVersionsMode::All) {
                get_padding_for_map(map_live_to_snaps)
            } else {
                0usize
            };
            parse_num_versions(
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

    Ok(write_out_buffer)
}

fn get_padding_for_map(map: &BTreeMap<PathData, Vec<PathData>>) -> usize {
    map.iter()
        .map(|(key, _values)| key)
        .max_by_key(|key| key.path_buf.to_string_lossy().len())
        .map_or_else(
            || QUOTATION_MARKS_LEN,
            |key| key.path_buf.to_string_lossy().len() + QUOTATION_MARKS_LEN,
        )
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
                _ => unreachable!(),
            }
        }
    }
}

pub fn display_mounts(config: &Config) -> HttmResult<()> {
    let mounts_for_files = get_mounts_for_files(config)?;

    display_as_map(config, mounts_for_files)?;

    Ok(())
}

pub fn display_as_map(config: &Config, map: BTreeMap<PathData, Vec<PathData>>) -> HttmResult<()> {
    let output_buf = if config.opt_raw || config.opt_zeros {
        display_raw(config, &map)?
    } else {
        display_map_formatted(config, &map)?
    };

    print_output_buf(output_buf)?;

    Ok(())
}

pub fn display_map_formatted(
    config: &Config,
    map: &BTreeMap<PathData, Vec<PathData>>,
) -> HttmResult<String> {
    let padding = get_padding_for_map(map);

    let write_out_buffer = map
        .iter()
        .filter(|(_key, values)| {
            if config.opt_last_snap.is_some() {
                !values.is_empty()
            } else {
                true
            }
        })
        .map(|(key, values)| {
            let display_path = if config.opt_no_pretty {
                key.path_buf.to_string_lossy().into()
            } else {
                format!("\"{}\"", key.path_buf.to_string_lossy())
            };

            let values_string: String = values
                .iter()
                .enumerate()
                .map(|(idx, value)| {
                    let value_string = value.path_buf.to_string_lossy();

                    if config.opt_no_pretty {
                        format!("{}{}", NOT_SO_PRETTY_FIXED_WIDTH_PADDING, value_string)
                    } else if idx == 0 {
                        format!(
                            "{:<width$} : \"{}\"\n",
                            display_path,
                            value_string,
                            width = padding
                        )
                    } else {
                        format!("{:<width$} : \"{}\"\n", "", value_string, width = padding)
                    }
                })
                .collect::<String>();

            if config.opt_no_pretty {
                format!("{}:{}\n", display_path, values_string)
            } else {
                values_string
            }
        })
        .collect();

    Ok(write_out_buffer)
}
