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

use crate::background::recursive::RecursiveSearch;
use crate::data::paths::PathData;
use crate::interactive::view_mode::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crate::GLOBAL_CONFIG;
use crossbeam_channel::unbounded;
use skim::prelude::*;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

#[derive(Debug)]
pub struct InteractiveBrowse {
    pub selected_pathdata: Vec<PathData>,
    pub opt_background_handle: Option<JoinHandle<()>>,
}

impl InteractiveBrowse {
    pub fn new() -> HttmResult<Self> {
        let browse_result = match &GLOBAL_CONFIG.opt_requested_dir {
            // collect string paths from what we get from lookup_view
            Some(requested_dir) => {
                let res = Self::view(requested_dir)?;

                if res.selected_pathdata.is_empty() {
                    return Err(HttmError::new(
                        "None of the selected strings could be converted to paths.",
                    )
                    .into());
                }

                res
            }
            None => {
                // go to interactive_select early if user has already requested a file
                // and we are in the appropriate mode Select or Restore, see struct Config,
                // and None here is also used for LastSnap to skip browsing for a file/dir
                match GLOBAL_CONFIG.paths.get(0) {
                    Some(first_path) => {
                        let selected_file = first_path.clone();

                        Self {
                            selected_pathdata: vec![selected_file],
                            opt_background_handle: None,
                        }
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

    #[allow(dead_code)]
    #[cfg(feature = "malloc_trim")]
    #[cfg(target_os = "linux")]
    #[cfg(target_env = "gnu")]
    fn malloc_trim() {
        unsafe {
            let _ = libc::malloc_trim(0usize);
        }
    }

    fn view(requested_dir: &Path) -> HttmResult<Self> {
        // prep thread spawn
        let started = Arc::new(AtomicBool::new(false));
        let hangup = Arc::new(AtomicBool::new(false));
        let hangup_clone = hangup.clone();
        let started_clone = started.clone();
        let requested_dir_clone = requested_dir.to_path_buf();
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();

        // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
        let background_handle = std::thread::spawn(move || {
            // no way to propagate error from closure so exit and explain error here
            RecursiveSearch::new(
                &requested_dir_clone,
                tx_item.clone(),
                hangup.clone(),
                started,
            )
            .exec();

            #[cfg(feature = "malloc_trim")]
            #[cfg(target_os = "linux")]
            #[cfg(target_env = "gnu")]
            Self::malloc_trim();
        });

        let header: String = ViewMode::Browse.print_header();

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
            .tiebreak(Some("score,index".to_string()))
            .algorithm(FuzzyAlgorithm::Simple)
            .build()
            .expect("Could not initialized skim options for browse_view");

        while !started_clone.load(Ordering::SeqCst) {}

        // run_with() reads and shows items from the thread stream created above
        match skim::Skim::run_with(&skim_opts, Some(rx_item)) {
            Some(output) if output.is_abort => {
                eprintln!("httm interactive file browse session was aborted.  Quitting.");
                std::process::exit(0)
            }
            Some(output) => {
                // hangup the channel so the background recursive search can gracefully cleanup and exit
                hangup_clone.store(true, Ordering::Release);

                #[cfg(feature = "malloc_trim")]
                #[cfg(target_os = "linux")]
                #[cfg(target_env = "gnu")]
                Self::malloc_trim();

                let selected_pathdata: Vec<PathData> = output
                    .selected_items
                    .iter()
                    .map(|item| PathData::from(Path::new(item.output().as_ref())))
                    .collect();

                Ok(Self {
                    selected_pathdata,
                    opt_background_handle: Some(background_handle),
                })
            }
            None => Err(HttmError::new("httm interactive file browse session failed.").into()),
        }
    }
}
