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
    DedupBy,
    FormattedMode,
    PrintMode,
};
use crate::data::paths::{
    BasicDirEntryInfo,
    PathData,
};
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::HttmResult;
use crate::library::utility::PaintString;
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
use std::fs::{
    FileType,
    Metadata,
};
use std::path::Path;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

static RETRY_NOTICE: &str = "NOTICE: httm filesystem requests are delayed...\n
We are probably waiting for your kernel auto-mounter to mount the snapshots which correspond to this file object.\n
Try again soon.  Number of retries you have left before this timeout is removed for this file object: ";

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
#[derive(Debug)]
pub struct SelectionCandidate {
    path: Box<Path>,
    display_name: Box<str>,
    opt_filetype: Option<FileType>,
    painted: Vec<u8>,
    count: AtomicU32,
}

impl Clone for SelectionCandidate {
    fn clone(&self) -> Self {
        SelectionCandidate {
            path: self.path.clone(),
            display_name: self.display_name.clone(),
            opt_filetype: self.opt_filetype.clone(),
            painted: self.painted.clone(),
            count: AtomicU32::default(),
        }
    }
}

impl From<BasicDirEntryInfo> for SelectionCandidate {
    fn from(value: BasicDirEntryInfo) -> Self {
        let painted = value.paint_string().to_string().into_bytes();

        SelectionCandidate {
            path: value.path().into(),
            display_name: value.display_name().into(),
            opt_filetype: value.opt_filetype().cloned(),
            painted,
            count: AtomicU32::default(),
        }
    }
}

impl SelectionCandidate {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn opt_filetype(&self) -> Option<&FileType> {
        self.opt_filetype.as_ref()
    }

    pub fn opt_metadata(&self) -> Option<Metadata> {
        self.path.symlink_metadata().ok()
    }

    fn preview_view(&self) -> HttmResult<String> {
        // generate a config for display
        let display_config: Config = Config::from(self);
        let display_path_data = [PathData::from(&self.path)];

        // finally run search on those paths
        let versions_map: VersionsMap = VersionsMap::new(&display_config, &display_path_data)?;

        let output_buf = DisplayWrapper::from(&display_config, versions_map).to_string();

        Ok(output_buf)
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_name)
    }
    fn display<'a>(&'a self, _context: DisplayContext) -> Line<'a> {
        let opt_text = self.painted.to_text().ok();

        opt_text
            .and_then(|text| text.into_iter().next())
            .unwrap_or_else(|| Line::from(self.text()))
    }
    fn output(&self) -> Cow<'_, str> {
        self.path.to_string_lossy()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        static REQUESTED_DIR_TIME_OUT: Duration = Duration::from_millis(1000);
        static REGULAR_TIME_OUT: Duration = Duration::from_millis(100);
        static MAX_RETRIES: u32 = 3u32;

        let time_out = match GLOBAL_CONFIG.opt_requested_dir.as_ref() {
            Some(requested_dir) if requested_dir.as_ref() == self.path() => REQUESTED_DIR_TIME_OUT,
            _ if GLOBAL_CONFIG.opt_lazy => REQUESTED_DIR_TIME_OUT,
            _ => REGULAR_TIME_OUT,
        };

        let retry_count = self.count.load(Ordering::Relaxed);

        if retry_count <= MAX_RETRIES {
            self.count.fetch_add(1, Ordering::Relaxed);

            let (s, r) = bounded(1);

            let cloned = self.clone();

            rayon::spawn(move || {
                let preview_output = cloned.preview_view().unwrap_or_default();
                let _ = s.send(preview_output);
            });

            match r.recv_timeout(time_out) {
                Ok(preview_output) => return skim::ItemPreview::AnsiText(preview_output),
                Err(_) => {
                    let retries_left = MAX_RETRIES - retry_count;
                    let err_output = format!("{}{}\n--", RETRY_NOTICE, retries_left);
                    return skim::ItemPreview::AnsiText(err_output);
                }
            }
        }

        let preview_output = self.preview_view().unwrap_or_default();
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
            dedup_by: DedupBy::Metadata,
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
