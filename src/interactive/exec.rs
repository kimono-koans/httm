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

use crate::config::generate::InteractiveMode;
use crate::data::paths::PathData;
use crate::interactive::browse::InteractiveBrowse;
use crate::interactive::select::InteractiveSelect;
use crate::library::results::HttmResult;

#[derive(Debug)]
pub struct InteractiveExec;

impl InteractiveExec {
    pub fn exec(interactive_mode: &InteractiveMode) -> HttmResult<Vec<PathData>> {
        let selected_pathdata = InteractiveBrowse::new()?;

        // do we return back to our main exec function to print,
        // or continue down the interactive rabbit hole?
        match interactive_mode {
            InteractiveMode::Restore(_) | InteractiveMode::Select(_) => {
                InteractiveSelect::exec(selected_pathdata, interactive_mode)?;
                unreachable!()
            }
            // InteractiveMode::Browse executes back through fn exec() in main.rs
            InteractiveMode::Browse => Ok(selected_pathdata),
        }
    }
}
