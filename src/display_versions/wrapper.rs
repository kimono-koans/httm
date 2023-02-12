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
use crate::display_other::wrapper::OtherDisplayWrapper;
use crate::lookup::versions::VersionsMap;

pub struct VersionsDisplayWrapper<'a> {
    pub config: &'a Config,
    pub map: VersionsMap,
}

impl<'a> VersionsDisplayWrapper<'a> {
    pub fn from(config: &'a Config, map: VersionsMap) -> Self {
        Self { config, map }
    }
}

impl<'a> std::string::ToString for VersionsDisplayWrapper<'a> {
    fn to_string(&self) -> String {
        match &self.config.exec_mode {
            ExecMode::NumVersions(num_versions_mode) => {
                self.map.format_as_num_versions(num_versions_mode)
            }
            ExecMode::Display if self.config.opt_last_snap.is_some() => {
                let printable_map = PrintAsMap::from(&self.map);
                OtherDisplayWrapper::from(self.config, printable_map).to_string()
            }
            ExecMode::Display if matches!(self.config.print_mode, PrintMode::FormattedJson) => {
                let printable_map = PrintAsMap::from(&self.map);
                printable_map.to_json()
            }
            _ => self.map.format(self.config),
        }
    }
}
