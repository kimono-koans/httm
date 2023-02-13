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

use crate::config::generate::{Config, ExecMode, PrintMode};
use crate::display_map::helper::PrintAsMap;
use crate::display_versions::format::NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
use crate::library::utility::get_delimiter;

pub struct OtherDisplayWrapper<'a> {
    pub config: &'a Config,
    pub map: PrintAsMap,
}

impl<'a> OtherDisplayWrapper<'a> {
    pub fn from(config: &'a Config, map: PrintAsMap) -> Self {
        Self { config, map }
    }
}

impl<'a> std::string::ToString for OtherDisplayWrapper<'a> {
    fn to_string(&self) -> String {
        match &self.config.print_mode {
            PrintMode::RawNewline | PrintMode::RawZero => self
                .map
                .values()
                .flatten()
                .map(|value| {
                    let delimiter = get_delimiter(self.config);
                    format!("{value}{delimiter}")
                })
                .collect::<String>(),
            PrintMode::FormattedJsonDefault | PrintMode::FormattedJsonNotPretty => {
                let json_string = self.to_json();

                match self.config.exec_mode {
                    ExecMode::Display | ExecMode::Interactive(_) => {
                        json_string.replace("\"inner\"", "\"versions\"")
                    }
                    ExecMode::MountsForFiles(_) => json_string.replace("\"inner\"", "\"mounts\""),
                    ExecMode::SnapsForFiles(_) => {
                        json_string.replace("\"inner\"", "\"snapshot_names\"")
                    }
                    ExecMode::NonInteractiveRecursive(_)
                    | ExecMode::NumVersions(_)
                    | ExecMode::Purge(_)
                    | ExecMode::SnapFileMount(_) => {
                        unreachable!("JSON print should not be available in the selected {:?} execution mode.", self.config.exec_mode);
                    }
                }
            }
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => self.format(),
        }
    }
}

impl<'a> OtherDisplayWrapper<'a> {
    pub fn to_json(&self) -> String {
        let res = match &self.config.print_mode {
            PrintMode::FormattedJsonNotPretty => serde_json::to_string(&self.map),
            _ => serde_json::to_string_pretty(&self.map),
        };

        match res {
            Ok(s) => s + "\n",
            Err(error) => {
                eprintln!("Error: {error}");
                std::process::exit(1)
            }
        }
    }

    pub fn format(&self) -> String {
        let padding = self.map.get_map_padding();

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
                        key.clone()
                    } else {
                        format!("\"{key}\"")
                    };

                let values_string: String = values
                    .iter()
                    .enumerate()
                    .map(|(idx, value)| {
                        if matches!(self.config.print_mode, PrintMode::FormattedNotPretty) {
                            format!("{NOT_SO_PRETTY_FIXED_WIDTH_PADDING}{value}")
                        } else if idx == 0 {
                            format!(
                                "{:<width$} : \"{}\"\n",
                                display_path,
                                value,
                                width = padding
                            )
                        } else {
                            format!("{:<padding$} : \"{value}\"\n", "")
                        }
                    })
                    .collect::<String>();

                if matches!(self.config.print_mode, PrintMode::FormattedNotPretty) {
                    format!("{display_path}:{values_string}\n")
                } else {
                    values_string
                }
            })
            .collect();

        write_out_buffer
    }
}
