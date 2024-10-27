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
use crate::{HttmResult, GLOBAL_CONFIG};
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

        // build our browse view - less to do than before - no previews, looking through one 'lil buffer
        let skim_opts = SkimOptionsBuilder::default()
            .preview_window(preview_selection.opt_preview_window.as_deref())
            .preview(preview_selection.opt_preview_command.as_deref())
            .disabled(true)
            .tac(true)
            .nosort(true)
            .tabstop(Some("4"))
            .exact(true)
            .multi(opt_multi)
            .regex(false)
            .tiebreak(Some("length,index".to_string()))
            .header(Some(&header))
            .build()
            .expect("Could not initialized skim options for select_restore_view");

        let item_reader_opts = SkimItemReaderOption::default().ansi(true);
        let item_reader = SkimItemReader::new(item_reader_opts);

        let (items, opt_ingest_handle) =
            item_reader.of_bufread(Box::new(Cursor::new(buffer.trim().to_owned())));

        // run_with() reads and shows items from the thread stream created above
        let res = match skim::Skim::run_with(&skim_opts, Some(items)) {
            Some(output) if output.is_abort => {
                eprintln!("httm select/restore/prune session was aborted.  Quitting.");
                std::process::exit(0);
            }
            Some(output) => output
                .selected_items
                .iter()
                .map(|i| i.output().into_owned())
                .collect(),
            None => {
                return Err(HttmError::new("httm select/restore/prune session failed.").into());
            }
        };

        if let Some(handle) = opt_ingest_handle {
            let _ = handle.join();
        };

        if GLOBAL_CONFIG.opt_debug {
            if let Some(preview_command) = preview_selection.opt_preview_command.as_deref() {
                eprintln!("DEBUG: Preview command executed: {}", preview_command)
            }
        }

        Ok(res)
    }
}
