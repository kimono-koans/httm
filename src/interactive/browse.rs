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

use crate::Config;
use crate::DisplayWrapper;
use crate::GLOBAL_CONFIG;
use crate::InteractiveSelect;
use crate::VersionsMap;
use crate::background::recursive::RecursiveSearch;
use crate::data::paths::PathData;
use crate::interactive::view_mode::MultiSelect;
use crate::interactive::view_mode::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crossbeam_channel::unbounded;
use skim::prelude::*;
use std::collections::HashSet;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{LazyLock, RwLock};

pub static CACHE_RESULT: LazyLock<Arc<RwLock<HashSet<PathBuf>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashSet::new())));

#[derive(Debug)]
pub struct InteractiveBrowse {
    selected_path_data: Vec<PathData>,
}

impl InteractiveBrowse {
    pub fn new() -> HttmResult<Self> {
        let browse_result = match &GLOBAL_CONFIG.opt_requested_dir {
            // collect string paths from what we get from lookup_view
            Some(requested_dir) => Self::view(requested_dir)?,
            None => {
                // go to interactive_select early if user has already requested a file
                // and we are in the appropriate mode Select or Restore, see struct Config,
                // and None here is also used for LastSnap to skip browsing for a file/dir
                match GLOBAL_CONFIG.paths.get(0) {
                    Some(first_path) => {
                        let selected_file = first_path.clone();

                        Self {
                            selected_path_data: vec![selected_file],
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

        {
            rayon::spawn(|| {
                match CACHE_RESULT.write() {
                    Ok(mut map) => map.clear(),
                    Err(_err) => {
                        CACHE_RESULT.clear_poison();
                    }
                };
            });
        }

        Ok(browse_result)
    }

    #[allow(dead_code)]
    #[cfg(feature = "malloc_trim")]
    #[cfg(target_os = "linux")]
    #[cfg(target_env = "gnu")]
    pub fn malloc_trim() {
        unsafe {
            let _ = libc::malloc_trim(0usize);
        }
    }

    fn view(requested_dir: &Path) -> HttmResult<Self> {
        // prep thread spawn
        let hangup = Arc::new(AtomicBool::new(false));
        let hangup_clone = hangup.clone();
        let requested_dir_clone = requested_dir.to_path_buf();
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();

        // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
        let background_handle = std::thread::spawn(move || {
            // no way to propagate error from closure so exit and explain error here
            RecursiveSearch::new(&requested_dir_clone, Some(&tx_item), hangup.clone()).exec();
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
            .tiebreak(Some("score,index,-length".to_string()))
            .algorithm(FuzzyAlgorithm::SkimV2)
            .build()
            .expect("Could not initialized skim options for browse_view");

        // run_with() reads and shows items from the thread stream created above
        match skim::Skim::run_with(&skim_opts, Some(rx_item)) {
            Some(output) if output.is_abort => {
                eprintln!("httm interactive file browse session was aborted.  Quitting.");
                std::process::exit(0)
            }
            Some(output) => {
                // hangup the channel so the background recursive search can gracefully cleanup and exit
                hangup_clone.store(true, Ordering::SeqCst);

                let selected_path_data: Vec<PathData> = output
                    .selected_items
                    .into_iter()
                    .map(|item| PathData::from(Path::new(item.output().as_ref())))
                    .collect();

                rayon::spawn(|| {
                    let _ = background_handle.join();

                    #[cfg(feature = "malloc_trim")]
                    #[cfg(target_os = "linux")]
                    #[cfg(target_env = "gnu")]
                    Self::malloc_trim();
                });

                if selected_path_data.is_empty() {
                    return HttmError::new(
                        "None of the selected strings could be converted to paths.",
                    )
                    .into();
                }

                Ok(Self { selected_path_data })
            }
            None => HttmError::new("httm interactive file browse session failed.").into(),
        }
    }

    pub fn selected_path_data(&self) -> &[PathData] {
        &self.selected_path_data
    }
}

impl TryInto<InteractiveSelect> for InteractiveBrowse {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_into(self) -> HttmResult<InteractiveSelect> {
        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &self.selected_path_data)?;

        // snap and live set has no snaps
        if versions_map.is_empty() {
            let paths: Vec<String> = self
                .selected_path_data
                .iter()
                .map(|path| path.path().to_string_lossy().to_string())
                .collect();
            let description = format!(
                "{}{:?}",
                "Cannot select or restore from the following paths as they have no snapshots:\n",
                paths
            );
            return HttmError::from(description).into();
        }

        let opt_live_version: Option<String> = if self.selected_path_data.len() > 1 {
            None
        } else {
            self.selected_path_data
                .get(0)
                .map(|path_data| path_data.path().to_string_lossy().into_owned())
        };

        let view_mode = ViewMode::Select(opt_live_version.clone());

        let snap_path_strings = if GLOBAL_CONFIG.opt_last_snap.is_some() {
            InteractiveSelect::last_snap(&versions_map)
        } else {
            // same stuff we do at fn exec, snooze...
            let display_config = Config::from(self.selected_path_data.as_slice());

            let display_map = DisplayWrapper::from(&display_config, versions_map);

            let selection_buffer = display_map.to_string();

            display_map.deref().iter().try_for_each(|(live, snaps)| {
                if snaps.is_empty() {
                    let description = format!("Path {:?} has no snapshots available.", live.path());
                    return HttmError::from(description).into();
                }

                Ok(())
            })?;

            // loop until user selects a valid snapshot version
            loop {
                // get the file name
                let selected_line = view_mode.view_buffer(&selection_buffer, MultiSelect::On)?;

                let requested_file_names = selected_line
                    .iter()
                    .filter_map(|selection| {
                        // ... we want everything between the quotes
                        selection
                            .split_once("\"")
                            .and_then(|(_lhs, rhs)| rhs.rsplit_once("\""))
                            .map(|(lhs, _rhs)| lhs)
                    })
                    .filter(|selection_buffer| {
                        // and cannot select a 'live' version or other invalid value.
                        display_map
                            .keys()
                            .all(|key| key.path() != Path::new(selection_buffer))
                    })
                    .map(|selection_buffer| selection_buffer.to_string())
                    .collect::<Vec<String>>();

                if requested_file_names.is_empty() {
                    continue;
                }

                break requested_file_names;
            }
        };

        Ok(InteractiveSelect::new(
            view_mode,
            snap_path_strings,
            opt_live_version,
        ))
    }
}
