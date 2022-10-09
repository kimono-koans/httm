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

use std::{borrow::Cow, collections::BTreeMap};

use number_prefix::NumberPrefix;
use terminal_size::{terminal_size, Height, Width};

use crate::config::init::{Config, ExecMode, NumVersionsMode};
use crate::data::filesystem_map::{DisplaySet, MapLiveToSnaps};
use crate::data::paths::{PathData, PHANTOM_DATE, PHANTOM_SIZE};
use crate::library::utility::{get_date, paint_string, print_output_buf, DateFormat, HttmResult};
use crate::lookup::file_mounts::get_mounts_for_files;

// 2 space wide padding - used between date and size, and size and path
const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = PRETTY_FIXED_WIDTH_PADDING.len() * 2;
// tab padding used in not so pretty
const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
const QUOTATION_MARKS_LEN: usize = 2;

struct PaddingCollection {
    size_padding_len: usize,
    fancy_border_string: String,
    phantom_date_pad_str: String,
    phantom_size_pad_str: String,
}

pub fn display_exec(config: &Config, map_live_to_snaps: &MapLiveToSnaps) -> HttmResult<String> {
    let output_buffer = if !matches!(config.opt_num_versions, NumVersionsMode::Disabled)
        || config.opt_raw
        || config.opt_zeros
    {
        display_raw(config, map_live_to_snaps)?
    } else {
        let display_set = map_to_display_set(config, map_live_to_snaps);
        display_formatted(config, &display_set)?
    };

    Ok(output_buffer)
}

fn display_raw(config: &Config, map_live_to_snaps: &MapLiveToSnaps) -> HttmResult<String> {
    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    let write_out_buffer = if !matches!(config.opt_num_versions, NumVersionsMode::Disabled) {
        map_live_to_snaps
            .iter()
            .filter_map(|(live_version, snaps)| {
                parse_num_versions(config, delimiter, live_version, snaps)
            })
            .collect()
    } else {
        let display_set = map_to_display_set(config, map_live_to_snaps);

        display_set
            .iter()
            .flatten()
            .map(|pathdata| {
                let display_path = pathdata.path_buf.display();
                format!("\"{}\"{}", display_path, delimiter)
            })
            .collect()
    };

    Ok(write_out_buffer)
}

fn parse_num_versions(
    config: &Config,
    delimiter: char,
    live_version: &PathData,
    snaps: &[PathData],
) -> Option<String> {
    let display_path = live_version.path_buf.display();

    if live_version.metadata.is_none() {
        return Some(format!(
            "\"{}\" : Path does not exist.{}",
            display_path, delimiter
        ));
    }

    let is_live_redundant = snaps.len() == 1
        && snaps
            .iter()
            .all(|snap_version| live_version.metadata == snap_version.metadata);

    match config.opt_num_versions {
        NumVersionsMode::All => {
            let num_versions = if !is_live_redundant {
                snaps.len() - 1
            } else {
                snaps.len()
            };

            Some(format!(
                "\"{}\" : {} Versions available.{}",
                display_path, num_versions, delimiter
            ))
        }
        NumVersionsMode::Multiple | NumVersionsMode::Single => {
            let is_only_version = snaps.is_empty() || is_live_redundant;

            match config.opt_num_versions {
                NumVersionsMode::Multiple => {
                    if is_only_version {
                        None
                    } else {
                        Some(format!("\"{}\"{}", display_path, delimiter))
                    }
                }
                NumVersionsMode::Single => {
                    if is_only_version {
                        Some(format!("\"{}\"{}", display_path, delimiter))
                    } else {
                        None
                    }
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

fn display_formatted(config: &Config, display_set: &DisplaySet) -> HttmResult<String> {
    let padding_collection = calculate_pretty_padding(config, display_set);

    let write_out_buffer = display_set.iter().enumerate().fold(
        String::new(),
        |mut write_out_buffer, (idx, pathdata_set)| {
            // a DisplaySet is an array of 2 - idx 0 are the snaps, 1 is the live versions
            let is_live_set = idx == 1;

            // get the display buffer for each set snaps and live
            let pathdata_set_buffer: String = pathdata_set
                .iter()
                .map(|pathdata| {
                    display_pathdata(config, pathdata, is_live_set, &padding_collection)
                })
                .collect();

            // add each buffer to the set - print fancy border string above, below and between sets
            if config.opt_no_pretty {
                write_out_buffer += &pathdata_set_buffer;
            } else if idx == 0 {
                write_out_buffer += &padding_collection.fancy_border_string;
                if !pathdata_set_buffer.is_empty() {
                    write_out_buffer += &pathdata_set_buffer;
                    write_out_buffer += &padding_collection.fancy_border_string;
                }
            } else if !pathdata_set.is_empty() {
                write_out_buffer += &pathdata_set_buffer;
                write_out_buffer += &padding_collection.fancy_border_string;
            }
            write_out_buffer
        },
    );

    Ok(write_out_buffer)
}

fn display_pathdata(
    config: &Config,
    pathdata: &PathData,
    is_live_set: bool,
    padding_collection: &PaddingCollection,
) -> String {
    // obtain metadata for timestamp and size
    let path_metadata = pathdata.md_infallible();

    // tab delimited if "no pretty", no border lines, and no colors
    let (display_size, display_path, display_padding) = if config.opt_no_pretty {
        // displays blanks for phantom values, equaling their dummy lens and dates.
        //
        // we use a dummy instead of a None value here.  Basically, sometimes, we want
        // to print the request even if a live file does not exist
        let size = if pathdata.metadata.is_some() {
            display_human_size(&path_metadata.size)
        } else {
            padding_collection.phantom_size_pad_str.to_owned()
        };
        let path = pathdata.path_buf.to_string_lossy();
        let padding = NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
        (size, path, padding)
    // print with padding and pretty border lines and ls colors
    } else {
        let size = {
            let size = if pathdata.metadata.is_some() {
                display_human_size(&path_metadata.size)
            } else {
                padding_collection.phantom_size_pad_str.to_owned()
            };
            format!(
                "{:>width$}",
                size,
                width = padding_collection.size_padding_len
            )
        };
        let path = {
            let path_buf = &pathdata.path_buf;
            // paint the live strings with ls colors - idx == 1 is 2nd or live set
            let painted_path_str = if is_live_set {
                paint_string(pathdata, path_buf.to_str().unwrap_or_default())
            } else {
                path_buf.to_string_lossy()
            };
            Cow::Owned(format!(
                "\"{:<width$}\"",
                painted_path_str,
                width = padding_collection.size_padding_len
            ))
        };
        // displays blanks for phantom values, equaling their dummy lens and dates.
        let padding = PRETTY_FIXED_WIDTH_PADDING;
        (size, path, padding)
    };

    let display_date = if pathdata.metadata.is_some() {
        get_date(config, &path_metadata.modify_time, DateFormat::Display)
    } else {
        padding_collection.phantom_date_pad_str.to_owned()
    };

    format!(
        "{}{}{}{}{}\n",
        display_date, display_padding, display_size, display_padding, display_path
    )
}

fn calculate_pretty_padding(config: &Config, display_set: &DisplaySet) -> PaddingCollection {
    // calculate padding and borders for display later
    let (size_padding_len, fancy_border_len) = display_set.iter().flatten().fold(
        (0usize, 0usize),
        |(mut size_padding_len, mut fancy_border_len), pathdata| {
            let path_metadata = pathdata.md_infallible();

            let (display_date, display_size, display_path) = {
                let date = get_date(config, &path_metadata.modify_time, DateFormat::Display);
                let size = format!(
                    "{:>width$}",
                    display_human_size(&path_metadata.size),
                    width = size_padding_len
                );
                let path = pathdata.path_buf.to_string_lossy();

                (date, size, path)
            };

            let display_size_len = display_human_size(&path_metadata.size).len();
            let formatted_line_len = display_date.len()
                + display_size.len()
                + display_path.len()
                + PRETTY_FIXED_WIDTH_PADDING_LEN_X2
                + QUOTATION_MARKS_LEN;

            size_padding_len = display_size_len.max(size_padding_len);
            fancy_border_len = formatted_line_len.max(fancy_border_len);
            (size_padding_len, fancy_border_len)
        },
    );

    let fancy_border_string: String = {
        let get_max_sized_border = || {
            // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
            // (0..fancy_border_len).map(|_| "─").collect()
            format!("{:─<width$}\n", "", width = fancy_border_len)
        };

        match terminal_size() {
            Some((Width(width), Height(_height))) => {
                if (width as usize) < fancy_border_len {
                    // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
                    // (0..width as usize).map(|_| "─").collect()
                    format!("{:─<width$}\n", "", width = width as usize)
                } else {
                    get_max_sized_border()
                }
            }
            None => get_max_sized_border(),
        }
    };

    let phantom_date_pad_str = format!(
        "{:<width$}",
        "",
        width = get_date(config, &PHANTOM_DATE, DateFormat::Display).len()
    );
    let phantom_size_pad_str = format!(
        "{:<width$}",
        "",
        width = display_human_size(&PHANTOM_SIZE).len()
    );

    PaddingCollection {
        size_padding_len,
        fancy_border_string,
        phantom_date_pad_str,
        phantom_size_pad_str,
    }
}

pub fn display_mounts_for_files(config: &Config) -> HttmResult<()> {
    let mounts_for_files = get_mounts_for_files(config)?;

    let output_buf = if config.opt_raw || config.opt_zeros {
        display_raw(config, &mounts_for_files)?
    } else {
        display_ordered_map(config, &mounts_for_files)?
    };

    print_output_buf(output_buf)?;

    Ok(())
}

fn display_ordered_map(
    config: &Config,
    map: &BTreeMap<PathData, Vec<PathData>>,
) -> HttmResult<String> {
    let write_out_buffer = if config.opt_no_pretty {
        map.iter()
            .map(|(key, values)| {
                let key_string = key.path_buf.to_string_lossy().to_string();

                let values_string: String = values
                    .iter()
                    .map(|value| {
                        format!(
                            "{}\"{}\"",
                            NOT_SO_PRETTY_FIXED_WIDTH_PADDING,
                            value.path_buf.to_string_lossy()
                        )
                    })
                    .collect();

                format!("{}:{}\n", key_string, values_string)
            })
            .collect()
    } else {
        let padding = map
            .iter()
            .map(|(key, _values)| key)
            .max_by_key(|key| key.path_buf.to_string_lossy().len())
            .map_or_else(|| 0usize, |key| key.path_buf.to_string_lossy().len());

        map.iter()
            .map(|(key, values)| {
                let key_string = key.path_buf.to_string_lossy();

                values
                    .iter()
                    .enumerate()
                    .map(|(idx, value)| {
                        let value_string = value.path_buf.to_string_lossy();

                        if idx == 0 {
                            format!(
                                "{:<width$} : \"{}\"\n",
                                key_string,
                                value_string,
                                width = padding
                            )
                        } else {
                            format!("{:<width$} : \"{}\"\n", "", value_string, width = padding)
                        }
                    })
                    .collect::<String>()
            })
            .collect()
    };

    Ok(write_out_buffer)
}

fn map_to_display_set(config: &Config, map_live_to_snaps: &MapLiveToSnaps) -> DisplaySet {
    let vec_snaps = if config.opt_no_snap {
        Vec::new()
    } else {
        map_live_to_snaps
            .clone()
            .into_iter()
            .flat_map(|(live_version, snaps)| {
                if config.opt_omit_identical {
                    snaps
                        .into_iter()
                        .filter(|snap_version| {
                            snap_version.metadata.is_some()
                                && snap_version.metadata != live_version.metadata
                        })
                        .collect()
                } else {
                    snaps
                }
            })
            .collect()
    };

    let vec_live = if config.opt_no_live || matches!(config.exec_mode, ExecMode::MountsForFiles) {
        Vec::new()
    } else {
        map_live_to_snaps.clone().into_keys().collect()
    };

    [vec_snaps, vec_live]
}

fn display_human_size(size: &u64) -> String {
    let size = *size as f64;

    match NumberPrefix::binary(size) {
        NumberPrefix::Standalone(bytes) => {
            format!("{} bytes", bytes)
        }
        NumberPrefix::Prefixed(prefix, n) => {
            format!("{:.1} {}B", n, prefix)
        }
    }
}
