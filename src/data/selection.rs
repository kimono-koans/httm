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

use crate::config::generate::{ListSnapsOfType, PrintMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::display_versions::wrapper::VersionsDisplayWrapper;
use crate::exec::recursive::PathProvenance;
use crate::library::results::HttmResult;
use crate::library::utility::paint_string;
use crate::{Config, ExecMode, VersionsMap, GLOBAL_CONFIG};
use lscolors::Colorable;
use once_cell::sync::Lazy;
use skim::prelude::*;
use std::fs::FileType;
use std::path::Path;
use std::path::PathBuf;

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    path: PathBuf,
    file_type: Option<FileType>,
}

impl SelectionCandidate {
    pub fn new(basic_info: BasicDirEntryInfo, is_phantom: PathProvenance) -> Self {
        // here save space of bool/padding instead of an "is_phantom: bool"
        //
        // issue: conflate not having a file_type as phantom
        // for purposes of coloring the file_name/path only?
        //
        // std lib docs don't give much indication as to
        // when file_type() fails?  Doesn't seem to be a problem?
        let file_type = match is_phantom {
            PathProvenance::FromLiveDataset => basic_info.file_type,
            PathProvenance::IsPhantom => None,
        };

        SelectionCandidate {
            path: basic_info.path,
            file_type,
        }
    }

    fn preview_view(&self) -> HttmResult<String> {
        // generate a config for display
        let display_config: Config = Config::from(self);

        // finally run search on those paths
        let versions_map = VersionsMap::new(&display_config, &display_config.paths)?;
        let output_buf = VersionsDisplayWrapper::from(&display_config, versions_map).to_string();

        Ok(output_buf)
    }

    fn display_name(&self) -> Cow<str> {
        static REQUESTED_DIR: Lazy<&Path> = Lazy::new(|| {
            GLOBAL_CONFIG
                .opt_requested_dir
                .as_ref()
                .expect("requested_dir should never be None in Interactive Browse mode")
        });

        // this only works because we do not resolve symlinks when doing traversal
        match self.path.strip_prefix(*REQUESTED_DIR) {
            Ok(stripped) if stripped.as_os_str().len() == 0 => Cow::Borrowed("."),
            Ok(stripped) => stripped.to_string_lossy(),
            Err(_) if Some(self.path.as_path()) == REQUESTED_DIR.parent() => Cow::Borrowed(".."),
            Err(_) => self.path.to_string_lossy(),
        }
    }
}

impl Colorable for &SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_os_string()
    }
    fn file_type(&self) -> Option<FileType> {
        self.file_type
    }
    fn metadata(&self) -> Option<std::fs::Metadata> {
        self.path.symlink_metadata().ok()
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<str> {
        self.path.to_string_lossy()
    }
    fn display(&self, _context: DisplayContext<'_>) -> AnsiString {
        AnsiString::parse(&paint_string(self, &self.display_name()))
    }
    fn output(&self) -> Cow<str> {
        self.text()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let preview_output = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(preview_output)
    }
}

impl From<Vec<PathData>> for Config {
    fn from(vec: Vec<PathData>) -> Config {
        let config = &GLOBAL_CONFIG;
        // generate a config for a preview display only
        Self {
            paths: vec,
            opt_recursive: false,
            opt_exact: false,
            opt_no_filter: false,
            opt_debug: false,
            opt_no_traverse: false,
            opt_no_hidden: false,
            opt_json: false,
            opt_one_filesystem: false,
            opt_no_clones: false,
            opt_bulk_exclusion: None,
            opt_last_snap: None,
            opt_preview: None,
            opt_deleted_mode: None,
            uniqueness: ListSnapsOfType::UniqueMetadata,
            opt_omit_ditto: config.opt_omit_ditto,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::BasicDisplay,
            print_mode: PrintMode::FormattedDefault,
            dataset_collection: config.dataset_collection.clone(),
            pwd: config.pwd.clone(),
            opt_requested_dir: config.opt_requested_dir.clone(),
        }
    }
}

impl From<&SelectionCandidate> for Config {
    fn from(selection_candidate: &SelectionCandidate) -> Config {
        let vec = vec![PathData::from(&selection_candidate.path)];

        Config::from(vec)
    }
}
