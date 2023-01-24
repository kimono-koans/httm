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

use crate::config::generate::{Config, PrintMode};
use crate::display::format::{NOT_SO_PRETTY_FIXED_WIDTH_PADDING, QUOTATION_MARKS_LEN};

pub trait ToStringMap {
    fn to_string_map(&self) -> BTreeMap<String, Vec<String>>;
}

pub type StringMap = BTreeMap<String, Vec<String>>;

pub fn get_map_padding(map: &StringMap) -> usize {
    map.iter()
        .map(|(key, _values)| key)
        .max_by_key(|key| key.len())
        .map_or_else(
            || QUOTATION_MARKS_LEN,
            |key| key.len() + QUOTATION_MARKS_LEN,
        )
}

pub fn format_as_map(map: &StringMap, config: &Config) -> String {
    let padding = get_map_padding(map);

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
            let display_path = if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                key.to_owned()
            } else {
                format!("\"{}\"", key)
            };

            let values_string: String = values
                .iter()
                .enumerate()
                .map(|(idx, value)| {
                    if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                        format!("{}{}", NOT_SO_PRETTY_FIXED_WIDTH_PADDING, value)
                    } else if idx == 0 {
                        format!(
                            "{:<width$} : \"{}\"\n",
                            display_path,
                            value,
                            width = padding
                        )
                    } else {
                        format!("{:<width$} : \"{}\"\n", "", value, width = padding)
                    }
                })
                .collect::<String>();

            if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                format!("{}:{}\n", display_path, values_string)
            } else {
                values_string
            }
        })
        .collect();

    write_out_buffer
}
