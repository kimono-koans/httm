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

use crate::config::generate::Config;
use crate::data::paths::PathData;
use crate::display::primary::{
    display_raw, NOT_SO_PRETTY_FIXED_WIDTH_PADDING, QUOTATION_MARKS_LEN,
};
use crate::library::results::HttmResult;
use crate::library::utility::print_output_buf;
use crate::lookup::file_mounts::MountsForFiles;

pub fn display_mounts(config: &Config) -> HttmResult<()> {
    let map = MountsForFiles::new(config);

    display_as_map(config, map)?;

    Ok(())
}

pub fn display_as_map(config: &Config, map: MountsForFiles) -> HttmResult<()> {
    let output_buf = if config.opt_raw || config.opt_zeros {
        display_raw(config, &map.into())
    } else {
        display_map_formatted(config, &map.into())
    };

    print_output_buf(output_buf)?;

    Ok(())
}

pub fn get_padding_for_map(map: &BTreeMap<PathData, Vec<PathData>>) -> usize {
    map.iter()
        .map(|(key, _values)| key)
        .max_by_key(|key| key.path_buf.to_string_lossy().len())
        .map_or_else(
            || QUOTATION_MARKS_LEN,
            |key| key.path_buf.to_string_lossy().len() + QUOTATION_MARKS_LEN,
        )
}

pub fn display_map_formatted(config: &Config, map: &BTreeMap<PathData, Vec<PathData>>) -> String {
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

    write_out_buffer
}
