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

use std::borrow::Cow;

use number_prefix::NumberPrefix;
use terminal_size::{terminal_size, Height, Width};

use crate::config::generate::{Config, ExecMode};
use crate::data::filesystem_map::{DisplaySet, MapLiveToSnaps};
use crate::data::paths::{PathData, PHANTOM_DATE, PHANTOM_SIZE};
use crate::display::special::display_num_versions;
use crate::library::results::HttmResult;
use crate::library::utility::{get_date, get_delimiter, paint_string, DateFormat};

// 2 space wide padding - used between date and size, and size and path
pub const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
pub const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = PRETTY_FIXED_WIDTH_PADDING.len() * 2;
// tab padding used in not so pretty
pub const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
pub const QUOTATION_MARKS_LEN: usize = 2;

struct PaddingCollection {
    size_padding_len: usize,
    fancy_border_string: String,
    phantom_date_pad_str: String,
    phantom_size_pad_str: String,
}

pub fn display_exec(config: &Config, map_live_to_snaps: &MapLiveToSnaps) -> HttmResult<String> {
    let output_buffer = match &config.exec_mode {
        ExecMode::NumVersions(num_versions_mode) => {
            display_num_versions(config, num_versions_mode, map_live_to_snaps)?
        }
        _ => {
            let drained_map: Vec<(&PathData, &Vec<PathData>)> = map_live_to_snaps.iter().collect();

            if config.opt_raw || config.opt_zeros || config.opt_last_snap.is_some() {
                display_raw(config, &drained_map)?
            } else {
                display_formatted(config, &drained_map)?
            }
        }
    };

    Ok(output_buffer)
}

pub fn display_raw(
    config: &Config,
    drained_map: &[(&PathData, &Vec<PathData>)],
) -> HttmResult<String> {
    let delimiter = get_delimiter(config);

    let write_out_buffer = drained_map
        .iter()
        .map(|(live_version, snaps)| {
            let display_set = get_display_set(config, &[(live_version, snaps)]);

            match config.opt_last_snap {
                None => display_set
                    .iter()
                    .flatten()
                    .map(|pathdata| format!("{}{}", pathdata.path_buf.display(), delimiter))
                    .collect::<String>(),
                Some(_) => {
                    // we need to index into this array for only the snaps
                    // flattening will zero out any evidence of a snap
                    display_set[0]
                        .iter()
                        .map(|pathdata| format!("{}{}", pathdata.path_buf.display(), delimiter))
                        .collect::<String>()
                }
            }
        })
        .collect();

    Ok(write_out_buffer)
}

fn display_formatted(
    config: &Config,
    drained_map: &[(&PathData, &Vec<PathData>)],
) -> HttmResult<String> {
    let global_display_set = get_display_set(config, drained_map);
    let global_padding_collection = calculate_pretty_padding(config, &global_display_set);

    let write_out_buffer = drained_map
        .iter()
        .map(|(live_version, snaps)| {
            // indexing safety: array has known len of 2
            if global_display_set[1].len() == 1 {
                global_display_set.clone()
            } else {
                let raw_instance_set = [(*live_version, *snaps)];
                get_display_set(config, &raw_instance_set)
            }
        })
        .map(|display_set| {
            // get the display buffer for each set snaps and live
            display_set.iter().enumerate().fold(
                String::new(),
                |mut display_set_buffer, (idx, snap_or_live_set)| {
                    // a DisplaySet is an array of 2 - idx 0 are the snaps, 1 is the live versions
                    let is_snap_set = idx == 0;
                    let is_live_set = idx == 1;

                    let component_buffer: String = snap_or_live_set
                        .iter()
                        .map(|pathdata| {
                            display_pathdata(
                                config,
                                pathdata,
                                is_live_set,
                                &global_padding_collection,
                            )
                        })
                        .collect();

                    // add each buffer to the set - print fancy border string above, below and between sets
                    if config.opt_no_pretty {
                        display_set_buffer += &component_buffer;
                    } else if is_snap_set {
                        display_set_buffer += &global_padding_collection.fancy_border_string;
                        if !component_buffer.is_empty() {
                            display_set_buffer += &component_buffer;
                            display_set_buffer += &global_padding_collection.fancy_border_string;
                        }
                    } else {
                        display_set_buffer += &component_buffer;
                        display_set_buffer += &global_padding_collection.fancy_border_string;
                    }
                    display_set_buffer
                },
            )
        })
        .collect();

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

pub fn get_display_set(config: &Config, drained_map: &[(&PathData, &Vec<PathData>)]) -> DisplaySet {
    let vec_snaps = if config.opt_no_snap {
        Vec::new()
    } else {
        drained_map
            .iter()
            .flat_map(|(_live_version, snaps)| *snaps)
            .cloned()
            .collect()
    };

    let vec_live = if config.opt_last_snap.is_some()
        || config.opt_no_live
        || matches!(config.exec_mode, ExecMode::MountsForFiles)
    {
        Vec::new()
    } else {
        drained_map
            .iter()
            .map(|(live_version, _snaps)| *live_version)
            .cloned()
            .collect()
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
