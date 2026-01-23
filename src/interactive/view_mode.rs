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

use crate::interactive::preview::PreviewSelection;
use crate::library::results::HttmError;
use crate::{
    GLOBAL_CONFIG,
    HttmResult,
    exit_success,
};
use skim::prelude::*;
use std::io::Cursor;

pub enum ViewMode {
    Browse,
    Select(Option<String>),
    Restore,
    Prune,
}

pub enum MultiSelect {
    On,
    Off,
}

impl ViewMode {
    pub fn print_header(&self) -> String {
        format!(
            "PREVIEW UP: shift+up | PREVIEW DOWN: shift+down | {}\n\
        PAGE UP:    page up  | PAGE DOWN:    page down \n\
        EXIT:       esc      | SELECT:       enter      | SELECT, MULTIPLE: shift+tab\n\
        ──────────────────────────────────────────────────────────────────────────────",
            self.print_mode()
        )
    }

    fn print_mode(&self) -> &str {
        match self {
            ViewMode::Browse => "====> [ Browse Mode ] <====",
            ViewMode::Select(_) => "====> [ Select Mode ] <====",
            ViewMode::Restore => "====> [ Restore Mode ] <====",
            ViewMode::Prune => "====> [ Prune Mode ] <====",
        }
    }

    pub fn view_buffer(&self, buffer: &str, opt_multi: MultiSelect) -> HttmResult<Vec<String>> {
        let preview_selection = PreviewSelection::new(&self)?;

        let header = self.print_header();

        let opt_multi = match opt_multi {
            MultiSelect::On => true,
            MultiSelect::Off => false,
        };

        let tiebreak = vec![
            RankCriteria::Score,
            RankCriteria::Index,
            RankCriteria::NegLength,
        ];

        // build our browse view - less to do than before - no previews, looking through one 'lil buffer
        let skim_opts = SkimOptionsBuilder::default()
            .preview_window(preview_selection.opt_preview_window())
            .preview(preview_selection.opt_preview_command())
            .disabled(true)
            .tac(true)
            .no_sort(true)
            .tabstop(4)
            .exact(true)
            .multi(opt_multi)
            .regex(false)
            .tiebreak(tiebreak)
            .header(Some(header))
            .header_lines(3)
            .build()
            .expect("Could not initialized skim options for select_restore_view");

        let item_reader_opts = SkimItemReaderOption::default().ansi(true);
        let item_reader = SkimItemReader::new(item_reader_opts);

        let items = item_reader.of_bufread(Box::new(Cursor::new(buffer.to_owned())));

        // run_with() reads and shows items from the thread stream created above
        let res = match skim::Skim::run_with(skim_opts, Some(items)) {
            Ok(output) if output.is_abort => {
                eprintln!("httm select/restore/prune session was aborted.  Quitting.");
                exit_success();
            }
            Ok(output) => output
                .selected_items
                .iter()
                .map(|i| i.output().into_owned())
                .collect(),
            Err(_) => {
                return HttmError::new("httm select/restore/prune session failed.").into();
            }
        };

        if GLOBAL_CONFIG.opt_debug {
            if let Some(preview_command) = preview_selection.opt_preview_command() {
                eprintln!("DEBUG: Preview command executed: {}", preview_command)
            }
        }

        Ok(res)
    }
}
