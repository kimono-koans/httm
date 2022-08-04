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

use crate::lookup_file_mounts::get_mounts_for_files;
use crate::utility::{get_date, paint_string, print_output_buf, DateFormat, PathData};
use crate::{Config, HttmResult, SnapsAndLiveSet};

// 2 space wide padding - used between date and size, and size and path
const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = PRETTY_FIXED_WIDTH_PADDING.len() * 2;
// tab padding used in not so pretty
const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
const QUOTATION_MARKS_LEN: usize = 2;

pub fn display_exec(config: &Config, snaps_and_live_set: &SnapsAndLiveSet) -> HttmResult<String> {
    let output_buffer = if config.opt_raw || config.opt_zeros {
        display_raw(config, snaps_and_live_set)?
    } else {
        display_pretty(config, snaps_and_live_set)?
    };

    Ok(output_buffer)
}

fn display_raw(config: &Config, snaps_and_live_set: &SnapsAndLiveSet) -> HttmResult<String> {
    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    // so easy!
    let write_out_buffer = snaps_and_live_set
        .iter()
        .flatten()
        .map(|pathdata| {
            let display_path = pathdata.path_buf.display();
            format!("{}{}", display_path, delimiter)
        })
        .collect();

    Ok(write_out_buffer)
}

fn display_pretty(config: &Config, snaps_and_live_set: &SnapsAndLiveSet) -> HttmResult<String> {
    let (size_padding_len, fancy_border_string) =
        calculate_pretty_padding(config, snaps_and_live_set);

    let write_out_buffer = snaps_and_live_set
        .iter()
        .enumerate()
        .map(|(idx, pathdata_set)| {
            let pathdata_set_buffer: String = pathdata_set
                .iter()
                .map(|pathdata| {
                    let path_metadata = pathdata.md_infallible();

                    // tab delimited if "no pretty", no border lines, and no colors
                    let (fmt_size, display_path, display_padding) = if config.opt_no_pretty {
                        let size = display_human_size(&path_metadata.size);
                        let path = pathdata.path_buf.to_string_lossy();
                        let padding = NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
                        (size, path, padding)
                    // print with padding and pretty border lines and ls colors
                    } else {
                        let size = format!(
                            "{:>width$}",
                            display_human_size(&path_metadata.size),
                            width = size_padding_len
                        );

                        let path = {
                            let file_path = &pathdata.path_buf;
                            // paint the live strings with ls colors - idx == 1 is 2nd or live set
                            let painted_path = if idx == 1 {
                                paint_string(pathdata, file_path.to_str().unwrap_or_default())
                            } else {
                                file_path.to_string_lossy()
                            };
                            Cow::Owned(format!(
                                "\"{:<width$}\"",
                                painted_path,
                                width = size_padding_len
                            ))
                        };

                        let padding = PRETTY_FIXED_WIDTH_PADDING;

                        (size, path, padding)
                    };

                    let fmt_date =
                        get_date(config, &path_metadata.modify_time, DateFormat::Display);

                    // displays blanks for phantom values, equaling their dummy lens and dates.
                    //
                    // we use a dummy instead of a None value here.  Basically, sometimes, we want
                    // to print the request even if a live file does not exist
                    let (display_date, display_size) = if pathdata.metadata.is_some() {
                        let date = fmt_date;
                        let size = fmt_size;
                        (date, size)
                    } else {
                        let date = format!("{:<width$}", "", width = fmt_size.len());
                        let size = format!("{:<width$}", "", width = fmt_date.len());
                        (date, size)
                    };

                    format!(
                        "{}{}{}{}{}\n",
                        display_date, display_padding, display_size, display_padding, display_path
                    )
                })
                .collect();

            if config.opt_no_pretty {
                pathdata_set_buffer
            } else {
                let mut pretty_buffer = String::new();
                if idx == 0 {
                    pretty_buffer += &fancy_border_string;
                    if !pathdata_set_buffer.is_empty() {
                        pretty_buffer += &pathdata_set_buffer;
                        pretty_buffer += &fancy_border_string;
                    }
                } else {
                    pretty_buffer += &pathdata_set_buffer;
                    pretty_buffer += &fancy_border_string;
                }
                pretty_buffer
            }
        })
        .collect();

    Ok(write_out_buffer)
}

fn calculate_pretty_padding(
    config: &Config,
    snaps_and_live_set: &SnapsAndLiveSet,
) -> (usize, String) {
    // calculate padding and borders for display later
    let (size_padding_len, fancy_border_len) = snaps_and_live_set.iter().flatten().fold(
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

    (size_padding_len, fancy_border_string)
}

pub fn display_mounts_for_files(config: &Config) -> HttmResult<()> {
    let mounts_for_files = get_mounts_for_files(config)?;

    let output_buf = if config.opt_raw || config.opt_zeros {
        display_exec(
            config,
            &[
                mounts_for_files.into_values().flatten().collect(),
                Vec::new(),
            ],
        )?
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
