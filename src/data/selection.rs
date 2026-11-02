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
use ratatui_core::text::Line;
use skim::prelude::*;
use std::path::Path;
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
    count: AtomicUsize,
}

impl Clone for SelectionCandidate {
    fn clone(&self) -> Self {
        SelectionCandidate {
            path: self.path.clone(),
            mid: self.mid.clone(),
            count: AtomicUsize::default(),
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
            count: AtomicUsize::default(),
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

    fn get_count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    fn increment_count(&self) -> usize {
        self.count.fetch_add(1, Ordering::Relaxed)
    }

    fn retries_exceeded(&self) -> bool {
        let retry_count = self.get_count();

        retry_count <= MAX_RETRIES
    }

    fn retry_notice(&self) -> String {
        let count = self.get_count();
        let retries_left = MAX_RETRIES - count;
        format!("{}{}\n--", PREVIEW_RETRY_NOTICE, retries_left)
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

        let time_out = match GLOBAL_CONFIG.opt_requested_dir.as_ref() {
            Some(requested_dir) if requested_dir.as_ref() == self.path() => REQUESTED_DIR_TIME_OUT,
            _ if GLOBAL_CONFIG.opt_lazy => REQUESTED_DIR_TIME_OUT,
            _ => REGULAR_TIME_OUT,
        };

        if self.retries_exceeded() {
            self.increment_count();

            let (s, r) = bounded(1);

            let cloned = self.clone();

            rayon::spawn(move || {
                let preview_output = cloned
                    .preview_view()
                    .unwrap_or_else(|err| SelectionCandidate::fail_notice(err));
                let _ = s.send(preview_output);
            });

            match r.recv_timeout(time_out) {
                Ok(preview_output) => return skim::ItemPreview::AnsiText(preview_output),
                Err(_) => return skim::ItemPreview::AnsiText(self.retry_notice()),
            }
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
