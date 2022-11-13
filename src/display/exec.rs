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

use crate::config::generate::{Config, ExecMode};
use crate::library::results::HttmResult;
use crate::library::utility::print_output_buf;
use crate::lookup::versions::DisplayMap;

impl DisplayMap {
    pub fn display(&self, config: &Config) -> HttmResult<String> {
        let output_buffer = match &config.exec_mode {
            ExecMode::NumVersions(num_versions_mode) => {
                self.print_num_versions(config, num_versions_mode)
            }
            _ => {
                if config.opt_raw || config.opt_zeros {
                    self.print_raw(config)
                } else {
                    self.print_formatted(config)
                }
            }
        };

        Ok(output_buffer)
    }

    pub fn display_as_map(&self, config: &Config) -> HttmResult<()> {
        let output_buf = if config.opt_raw || config.opt_zeros {
            self.print_raw(config)
        } else {
            self.print_formatted_map(config)
        };

        print_output_buf(output_buf)
    }
}
