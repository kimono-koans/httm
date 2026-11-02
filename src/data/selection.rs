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

use crate::config::generate::{
    FormattedMode,
    PrintMode,
};
use crate::data::paths::{
    BasicDirEntryInfo,
    PathData,
    PathDeconstruction,
};
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::{
    HttmGenericErrorWrapper,
    HttmResult,
};
use crate::library::utility::PaintPath;
use crate::{
    Config,
    ExecMode,
    GLOBAL_CONFIG,
    VersionsMap,
};
use ansi_to_tui::IntoText;
use crossbeam_channel::bounded;
use hashbrown::HashMap;
use ratatui_core::text::Line;
use skim::prelude::*;
use std::path::Path;
use std::sync::{
    LazyLock,
    Mutex,
};
use std::time::Duration;

static MAX_RETRIES: usize = 3usize;

static PREVIEW_RETRY_NOTICE: &str = "NOTICE: httm filesystem requests are delayed...\n
We are probably waiting for your kernel auto-mounter to mount the snapshots which correspond to this file object.\n
Try again soon.  Number of retries you have left before this timeout is removed for this file object: ";

static PREVIEW_FAILURE_NOTICE: &str =
    "NOTICE: httm could not render a preview for the selected path for the following reason...";

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
#[derive(Debug)]
pub struct SelectionCandidate {
    path: Box<[u8]>,
    mid: usize,
}

impl Clone for SelectionCandidate {
    fn clone(&self) -> Self {
        SelectionCandidate {
            path: self.path.clone(),
            mid: self.mid.clone(),
        }
    }
}

impl From<BasicDirEntryInfo> for SelectionCandidate {
    fn from(value: BasicDirEntryInfo) -> Self {
        let mut all_bytes = value.path().as_os_str().as_encoded_bytes().to_vec();
        let mid = all_bytes.len();
        let painted = value.paint_path();

        all_bytes.extend_from_slice(painted.to_string().as_bytes());

        SelectionCandidate {
            path: all_bytes.into(),
            mid,
        }
    }
}

impl SelectionCandidate {
    pub fn path(&self) -> &Path {
        let slice = self.path.split_at(self.mid).0;

        // SAFETY: "unsafe fn from_utf_unchecked" is functionally the same, but calling it seems to anger the borrow checker gods
        // Bytes were already been UTF-8 validated when string was painted and then converted into bytes from path.
        let ret: &str = unsafe { &*(slice as *const [u8] as *const str) };

        Path::new(ret)
    }

    pub fn painted_slice(&self) -> &[u8] {
        self.path.split_at(self.mid).1
    }

    fn preview_view(&self) -> HttmResult<String> {
        // generate a config for display
        let display_config: Config = Config::from(self);
        let display_path_data = display_config.paths.as_slice();

        // finally run search on those paths
        let versions_map: VersionsMap = VersionsMap::new(&display_config, &display_path_data)?;

        let output_buf = DisplayWrapper::from(&display_config, versions_map).to_string();

        Ok(output_buf)
    }

    fn fail_notice(err: HttmGenericErrorWrapper) -> String {
        format!("{}\nERROR: {}\n--", PREVIEW_FAILURE_NOTICE, err)
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<'_, str> {
        let slice = self.painted_slice();

        // SAFETY: "unsafe fn from_utf_unchecked" is functionally the same, but calling it seems to anger the borrow checker gods
        // Bytes were already been UTF-8 validated when string was painted and then converted into bytes from path.
        let ret: &str = unsafe { &*(slice as *const [u8] as *const str) };

        Cow::Borrowed(ret)
    }

    fn display<'a>(&'a self, _context: DisplayContext) -> Line<'a> {
        self.painted_slice()
            .into_text()
            .ok()
            .and_then(|inner| inner.into_iter().next())
            .unwrap_or_else(|| Line::from(self.text()))
    }

    fn output(&self) -> Cow<'_, str> {
        self.path().to_string_lossy()
    }

    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        static REQUESTED_DIR_TIME_OUT: Duration = Duration::from_millis(1000);
        static REGULAR_TIME_OUT: Duration = Duration::from_millis(100);
        static CACHE: LazyLock<Arc<Mutex<DelayCache>>> = LazyLock::new(|| {
            let map = DelayCache::new();

            Arc::new(Mutex::new(map))
        });

        let time_out = match GLOBAL_CONFIG.opt_requested_dir.as_ref() {
            Some(requested_dir) if requested_dir.as_ref() == self.path() => REQUESTED_DIR_TIME_OUT,
            _ if GLOBAL_CONFIG.opt_lazy => REQUESTED_DIR_TIME_OUT,
            _ => REGULAR_TIME_OUT,
        };

        let pathdata: PathData = self.path().into();

        if let Ok(mut locked) = CACHE.lock() {
            if locked.retries_not_exceeded(&pathdata) {
                let (s, r) = bounded(1);

                let cloned = self.clone();

                rayon::spawn(move || {
                    let preview_output = cloned
                        .preview_view()
                        .unwrap_or_else(|err| SelectionCandidate::fail_notice(err));
                    let _ = s.send(preview_output);
                });

                match r.recv_timeout(time_out) {
                    Ok(preview_output) => {
                        locked.remove_entry(&pathdata);

                        return skim::ItemPreview::AnsiText(preview_output);
                    }
                    Err(_) => {
                        return skim::ItemPreview::AnsiText(locked.retry_notice(&pathdata));
                    }
                }
            }

            let preview_output = self
                .preview_view()
                .map(|res| {
                    let _ = locked.remove_entry(&pathdata);
                    res
                })
                .unwrap_or_else(|err| SelectionCandidate::fail_notice(err));

            return skim::ItemPreview::AnsiText(preview_output);
        }

        let preview_output = self
            .preview_view()
            .unwrap_or_else(|err| SelectionCandidate::fail_notice(err));

        skim::ItemPreview::AnsiText(preview_output)
    }
}

impl From<&[PathData]> for Config {
    fn from(slice: &[PathData]) -> Config {
        let config = &GLOBAL_CONFIG;

        // generate a config for a preview display only
        Self {
            paths: slice.to_vec(),
            opt_recursive: false,
            opt_exact: false,
            opt_no_filter: false,
            opt_debug: false,
            opt_no_traverse: false,
            opt_no_hidden: false,
            opt_json: false,
            opt_one_filesystem: false,
            opt_no_clones: false,
            opt_lazy: config.opt_lazy,
            opt_bulk_exclusion: None,
            opt_last_snap: None,
            opt_preview: None,
            opt_deleted_mode: None,
            opt_dedup_by: None,
            opt_omit_ditto: config.opt_omit_ditto,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::Preview,
            print_mode: PrintMode::Formatted(FormattedMode::Default),
            dataset_collection: config.dataset_collection.clone(),
            pwd: config.pwd.clone(),
            opt_requested_dir: config.opt_requested_dir.clone(),
            opt_preheat_cache: config.opt_preheat_cache.clone(),
        }
    }
}

impl From<&SelectionCandidate> for Config {
    fn from(selection_candidate: &SelectionCandidate) -> Config {
        let vec = [PathData::from(selection_candidate)];

        Config::from(vec.as_slice())
    }
}

struct DelayCache {
    inner: HashMap<Arc<Path>, usize>,
    is_empty: bool,
}

impl DelayCache {
    fn new() -> Self {
        let opt_prox = GLOBAL_CONFIG
            .opt_requested_dir
            .as_ref()
            .map(|requested_dir| PathData::from(&requested_dir))
            .and_then(|pd| pd.proximate_dataset().ok().map(|path| path.to_path_buf()));

        let map: HashMap<Arc<Path>, usize> = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .keys()
            .cloned()
            .filter_map(|key| match &opt_prox {
                Some(prox) if key.starts_with(prox) => Some(key),
                Some(_) => None,
                None => Some(key),
            })
            .map(|key| (key, 0usize))
            .collect();

        let is_empty = map.is_empty();

        Self {
            inner: map,
            is_empty,
        }
    }

    fn get_count(&self, pathdata: &PathData) -> Option<usize> {
        pathdata
            .proximate_dataset()
            .ok()
            .and_then(|prox| self.inner.get(prox).cloned())
    }

    fn increment_count(&mut self, pathdata: &PathData) -> Option<usize> {
        pathdata
            .proximate_dataset()
            .ok()
            .and_then(|prox| self.inner.get_mut(prox))
            .map(|count| {
                *count += 1;
                *count
            })
    }

    fn remove_entry(&mut self, pathdata: &PathData) -> Option<(Arc<Path>, usize)> {
        pathdata
            .proximate_dataset()
            .ok()
            .and_then(|prox| self.inner.remove_entry(prox))
    }

    fn retries_not_exceeded(&mut self, pathdata: &PathData) -> bool {
        let is_empty = self.is_empty || self.inner.is_empty();

        if is_empty {
            if !self.is_empty {
                self.is_empty = is_empty;
            }

            return false;
        }

        let Some(retry_count) = self.get_count(pathdata) else {
            return false;
        };

        if self.increment_count(&pathdata).is_none() {
            return false;
        }

        retry_count <= MAX_RETRIES
    }

    fn retry_notice(&self, pathdata: &PathData) -> String {
        let count = self.get_count(pathdata);
        let retries_left = MAX_RETRIES - count.unwrap_or_else(|| MAX_RETRIES);
        format!("{}{}\n--", PREVIEW_RETRY_NOTICE, retries_left)
    }
}
