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

use crate::background::recursive::PathProvenance;
use crate::config::generate::{DedupBy, FormattedMode, PrintMode};
use crate::data::paths::BasicDirEntryInfo;
use crate::data::paths::PathData;
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::HttmResult;
use crate::library::utility::PaintString;
use crate::{Config, ExecMode, GLOBAL_CONFIG, VersionsMap};
use lscolors::Colorable;
use skim::prelude::*;
use std::collections::BTreeMap;
use std::fs::FileType;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    path: Box<Path>,
    opt_filetype: Option<FileType>,
}

impl SelectionCandidate {
    pub fn new(basic_dir_entry_info: BasicDirEntryInfo, path_provenance: &PathProvenance) -> Self {
        let mut res: Self = unsafe { std::mem::transmute(basic_dir_entry_info) };

        if let PathProvenance::IsPhantom = path_provenance {
            res.opt_filetype = None;
        }

        res
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn opt_filetype(&self) -> Option<FileType> {
        self.opt_filetype
    }

    fn preview_view(&self) -> HttmResult<String> {
        // generate a config for display
        let display_config: Config = Config::from(self);
        let display_path_data = PathData::from(&self.path);
        static IS_INTERACTIVE_MODE: bool = true;

        // finally run search on those paths
        let versions_map: VersionsMap =
            VersionsMap::from_one_path(&display_config, &display_path_data, IS_INTERACTIVE_MODE)
                .into_iter()
                .map(|versions| versions.into_inner())
                .collect::<BTreeMap<PathData, Vec<PathData>>>()
                .into();

        let output_buf = DisplayWrapper::from(&display_config, versions_map).to_string();

        Ok(output_buf)
    }

    pub fn display_name(&self) -> Cow<str> {
        static PWD_DIR: LazyLock<&Path> = LazyLock::new(|| GLOBAL_CONFIG.pwd.as_ref());

        static PWD_DIR_PARENT: LazyLock<Option<&Path>> =
            LazyLock::new(|| GLOBAL_CONFIG.pwd.parent());

        // this only works because we do not resolve symlinks when doing traversal
        match self.path.strip_prefix(*PWD_DIR) {
            Ok(_) if self.path.as_ref() == *PWD_DIR => Cow::Borrowed("."),
            Ok(stripped) => stripped.to_string_lossy(),
            Err(_) if Some(self.path.as_ref()) == *PWD_DIR_PARENT => Cow::Borrowed(".."),
            Err(_) => self.path.to_string_lossy(),
        }
    }
}

impl Colorable for SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.to_path_buf()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_os_string()
    }
    fn file_type(&self) -> Option<FileType> {
        self.opt_filetype()
    }
    fn metadata(&self) -> Option<std::fs::Metadata> {
        self.path.symlink_metadata().ok()
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<str> {
        self.display_name()
    }
    fn display(&self, _context: DisplayContext<'_>) -> AnsiString {
        AnsiString::parse(&self.paint_string().to_string())
    }
    fn output(&self) -> Cow<str> {
        self.path.to_string_lossy()
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
            opt_lazy: config.opt_lazy,
            opt_bulk_exclusion: None,
            opt_last_snap: None,
            opt_preview: None,
            opt_deleted_mode: None,
            dedup_by: DedupBy::Metadata,
            opt_omit_ditto: config.opt_omit_ditto,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::BasicDisplay,
            print_mode: PrintMode::Formatted(FormattedMode::Default),
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
