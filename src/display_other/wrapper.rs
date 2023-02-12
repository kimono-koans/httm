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

use crate::display_other::generic_maps::PrintAsMap;
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
                let json_string = self.map.to_json(self.config);

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
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                self.map.format(self.config)
            }
        }
    }
}
