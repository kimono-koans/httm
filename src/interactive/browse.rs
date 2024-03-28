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
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::data::paths::PathData;
use crate::interactive::view_mode::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crate::GLOBAL_CONFIG;

#[derive(Debug)]
pub struct InteractiveBrowse;

impl InteractiveBrowse {
    pub fn new() -> HttmResult<Vec<PathData>> {
        let browse_result = match &GLOBAL_CONFIG.opt_requested_dir {
            // collect string paths from what we get from lookup_view
            Some(requested_dir) => {
                let view_mode = ViewMode::Browse;
                let selected_pathdata = view_mode.browse(requested_dir)?;

                if selected_pathdata.is_empty() {
                    return Err(HttmError::new(
                        "None of the selected strings could be converted to paths.",
                    )
                    .into());
                }

                selected_pathdata
            }
            None => {
                // go to interactive_select early if user has already requested a file
                // and we are in the appropriate mode Select or Restore, see struct Config,
                // and None here is also used for LastSnap to skip browsing for a file/dir
                match GLOBAL_CONFIG.paths.get(0) {
                    Some(first_path) => {
                        let selected_file = first_path.clone();

                        vec![selected_file]
                    }
                    // Config::from should never allow us to have an instance where we don't
                    // have at least one path to use
                    None => unreachable!(
                        "GLOBAL_CONFIG.paths.get(0) should never be a None value in Interactive Mode"
                    ),
                }
            }
        };

        Ok(browse_result)
    }
}
