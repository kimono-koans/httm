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
use crate::data::paths::PathData;
use crate::library::results::HttmResult;

use crate::display_other::generic_maps::PrintableMap;
use crate::lookup::versions::VersionsMap;

pub struct VersionsDisplayWrapper<'a> {
    pub config: &'a Config,
    pub map: VersionsMap,
}

impl<'a> VersionsDisplayWrapper<'a> {
    pub fn new(config: &'a Config, path_set: &'a [PathData]) -> HttmResult<Self> {
        let map = VersionsMap::new(config, path_set)?;

        Ok(Self { config, map })
    }

    pub fn from(config: &'a Config, map: VersionsMap) -> Self {
        Self { config, map }
    }
}

impl<'a> std::string::ToString for VersionsDisplayWrapper<'a> {
    fn to_string(&self) -> String {
        match &self.config.exec_mode {
            ExecMode::NumVersions(num_versions_mode) => self
                .map
                .format_as_num_versions(self.config, num_versions_mode),
            ExecMode::Display
                if self.config.opt_last_snap.is_some()
                    && !matches!(
                        self.config.print_mode,
                        PrintMode::RawNewline | PrintMode::RawZero
                    ) =>
            {
                let printable_map = PrintableMap::from(&self.map);
                printable_map.format_as_map(self.config)
            }
            _ => self.map.format(self.config),
        }
    }
}
