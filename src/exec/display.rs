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

use std::ops::Deref;

use crate::config::generate::{Config, ExecMode, PrintMode};
use crate::display::primary::{DisplaySet, PaddingCollection};
use crate::library::utility::get_delimiter;
use crate::lookup::versions::VersionsMap;

impl VersionsMap {
    pub fn display(&self, config: &Config) -> String {
        match &config.exec_mode {
            ExecMode::NumVersions(num_versions_mode) => {
                self.format_as_num_versions(config, num_versions_mode)
            }
            ExecMode::Display | ExecMode::MountsForFiles
                if config.opt_last_snap.is_some()
                    && !matches!(
                        config.print_mode,
                        PrintMode::RawNewline | PrintMode::RawZero
                    ) =>
            {
                self.format_as_map(config)
            }
            _ => self.format(config),
        }
    }

    fn format(&self, config: &Config) -> String {
        let global_display_set = DisplaySet::new(config, self);
        let padding_collection = PaddingCollection::new(config, &global_display_set);

        match &config.print_mode {
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty if self.len() == 1 => {
                global_display_set.format(config, &padding_collection)
            }
            _ => self
                .deref()
                .clone()
                .into_iter()
                .map(std::convert::Into::into)
                .map(|raw_instance_set| DisplaySet::new(config, &raw_instance_set))
                .map(|display_set| match config.print_mode {
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        display_set.format(config, &padding_collection)
                    }
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        let delimiter = get_delimiter(config);
                        display_set
                            .iter()
                            .flatten()
                            .map(|pathdata| format!("{}{}", pathdata.path_buf.display(), delimiter))
                            .collect()
                    }
                })
                .collect::<String>(),
        }
    }
}
