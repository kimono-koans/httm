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

use crate::config::generate::{Config, PrintMode};

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
            PrintMode::FormattedJson => self.map.to_json(),
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                self.map.format(self.config)
            }
        }
    }
}
