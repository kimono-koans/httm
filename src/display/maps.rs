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

use crate::config::generate::PrintMode;
use crate::display::format::{NOT_SO_PRETTY_FIXED_WIDTH_PADDING, QUOTATION_MARKS_LEN};
use crate::exec::display::DisplayWrapper;

impl<'a> DisplayWrapper<'a> {
    pub fn get_map_padding(&self) -> usize {
        self.map
            .iter()
            .map(|(key, _values)| key)
            .max_by_key(|key| key.path_buf.to_string_lossy().len())
            .map_or_else(
                || QUOTATION_MARKS_LEN,
                |key| key.path_buf.to_string_lossy().len() + QUOTATION_MARKS_LEN,
            )
    }

    pub fn format_as_map(&self) -> String {
        let padding = self.get_map_padding();

        let write_out_buffer = self
            .map
            .iter()
            .filter(|(_key, values)| {
                if self.config.opt_last_snap.is_some() {
                    !values.is_empty()
                } else {
                    true
                }
            })
            .map(|(key, values)| {
                let display_path =
                    if matches!(self.config.print_mode, PrintMode::FormattedNotPretty) {
                        key.path_buf.to_string_lossy().into()
                    } else {
                        format!("\"{}\"", key.path_buf.to_string_lossy())
                    };

                let values_string: String = values
                    .iter()
                    .enumerate()
                    .map(|(idx, value)| {
                        let value_string = value.path_buf.to_string_lossy();

                        if matches!(self.config.print_mode, PrintMode::FormattedNotPretty) {
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

                if matches!(self.config.print_mode, PrintMode::FormattedNotPretty) {
                    format!("{}:{}\n", display_path, values_string)
                } else {
                    values_string
                }
            })
            .collect();

        write_out_buffer
    }
}
