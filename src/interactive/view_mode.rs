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
use crate::exec::preview::PreviewSelection;
use crate::exec::recursive::RecursiveSearch;
use crate::interactive::browse::InteractiveBrowse;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::Never;
use crate::GLOBAL_CONFIG;
use crossbeam_channel::unbounded;
use skim::prelude::*;
use std::io::Cursor;
use std::path::Path;
use std::thread;

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
    fn print_header(&self) -> String {
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

    pub fn browse(&self, requested_dir: &Path) -> HttmResult<InteractiveBrowse> {
        // prep thread spawn
        let requested_dir_clone = requested_dir.to_path_buf();
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
        let (hangup_tx, hangup_rx): (Sender<Never>, Receiver<Never>) = bounded(0);

        // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
        let background_handle = thread::spawn(move || {
            // no way to propagate error from closure so exit and explain error here
            RecursiveSearch::exec(&requested_dir_clone, tx_item.clone(), hangup_rx.clone());
        });

        let header: String = self.print_header();

        let display_handle = thread::spawn(move || {
            #[cfg(feature = "setpriority")]
            #[cfg(target_os = "linux")]
            #[cfg(target_env = "gnu")]
            {
                use crate::library::utility::ThreadPriorityType;
                let tid = std::process::id();
                let _ = ThreadPriorityType::Process.nice_thread(Some(tid), -3i32);
            }

            let opt_multi = GLOBAL_CONFIG.opt_preview.is_none();

            // create the skim component for previews
            let skim_opts = SkimOptionsBuilder::default()
                .preview_window(Some("up:50%"))
                .preview(Some(""))
                .nosort(true)
                .exact(GLOBAL_CONFIG.opt_exact)
                .header(Some(&header))
                .multi(opt_multi)
                .regex(false)
                .build()
                .expect("Could not initialized skim options for browse_view");

            // run_with() reads and shows items from the thread stream created above
            let res = match skim::Skim::run_with(&skim_opts, Some(rx_item)) {
                Some(output) if output.is_abort => {
                    eprintln!("httm interactive file browse session was aborted.  Quitting.");
                    std::process::exit(0)
                }
                Some(output) => {
                    // hangup the channel so the background recursive search can gracefully cleanup and exit
                    drop(hangup_tx);

                    output
                        .selected_items
                        .iter()
                        .map(|i| PathData::from(Path::new(&i.output().to_string())))
                        .collect()
                }
                None => {
                    return Err(HttmError::new(
                        "httm interactive file browse session failed.",
                    ));
                }
            };

            #[cfg(feature = "malloc_trim")]
            #[cfg(target_os = "linux")]
            #[cfg(target_env = "gnu")]
            {
                use crate::library::utility::malloc_trim;
                malloc_trim();
            }

            Ok(res)
        });

        match display_handle.join() {
            Ok(selected_pathdata) => {
                let res = InteractiveBrowse {
                    selected_pathdata: selected_pathdata?,
                    opt_background_handle: Some(background_handle),
                };
                Ok(res)
            }
            Err(_) => Err(HttmError::new("Interactive browse thread panicked.").into()),
        }
    }

    pub fn select(&self, preview_buffer: &str, opt_multi: MultiSelect) -> HttmResult<Vec<String>> {
        let preview_selection = PreviewSelection::new(self)?;

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
            item_reader.of_bufread(Box::new(Cursor::new(preview_buffer.trim().to_owned())));

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
