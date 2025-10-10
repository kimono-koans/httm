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
use crate::data::paths::PathData;
use crate::display::wrapper::DisplayWrapper;
use crate::library::results::HttmResult;
use crate::library::utility::{ENV_LS_COLORS, PaintString};
use crate::{Config, ExecMode, GLOBAL_CONFIG, VersionsMap};
use lscolors::Colorable;
use lscolors::Style;
use skim::prelude::*;
use std::collections::BTreeMap;
use std::fs::{FileType, Metadata};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock};

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    path: Box<Path>,
    opt_filetype: Option<FileType>,
    opt_style: OnceLock<Option<Style>>,
    opt_metadata: OnceLock<Option<Metadata>>,
}

impl Colorable for &SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.to_path_buf()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_os_string()
    }
    fn file_type(&self) -> Option<FileType> {
        self.opt_filetype().copied()
    }
    fn metadata(&self) -> Option<std::fs::Metadata> {
        self.opt_metadata().cloned()
    }
}

impl SelectionCandidate {
    pub fn new(
        path: Box<Path>,
        opt_filetype: Option<FileType>,
        opt_style: Option<Style>,
        opt_metadata: Option<Metadata>,
        path_provenance: &PathProvenance,
    ) -> Self {
        let style = if opt_style.is_some() {
            OnceLock::from(opt_style)
        } else {
            OnceLock::new()
        };

        let md = if opt_metadata.is_some() {
            OnceLock::from(opt_metadata)
        } else {
            OnceLock::new()
        };

        let mut res: Self = Self {
            path,
            opt_filetype,
            opt_style: style,
            opt_metadata: md,
        };

        if let PathProvenance::IsPhantom = path_provenance {
            res.opt_filetype = None;
            res.opt_metadata = OnceLock::from(None);
            res.opt_style = OnceLock::from(None);
        }

        res
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn opt_filetype(&self) -> Option<&FileType> {
        self.opt_filetype.as_ref()
    }

    pub fn opt_style(&self) -> Option<Style> {
        *self
            .opt_style
            .get_or_init(|| ENV_LS_COLORS.style_for(&self).copied())
    }

    pub fn opt_metadata(&self) -> Option<&Metadata> {
        self.opt_metadata
            .get_or_init(|| self.path().symlink_metadata().ok())
            .as_ref()
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

    pub fn display_name(&self) -> Cow<'_, str> {
        static REQUESTED_DIR: LazyLock<&Path> = LazyLock::new(|| {
            GLOBAL_CONFIG
                .opt_requested_dir
                .as_ref()
                .unwrap_or_else(|| &GLOBAL_CONFIG.pwd)
                .as_ref()
        });

        static REQUESTED_DIR_PARENT: LazyLock<Option<&Path>> =
            LazyLock::new(|| REQUESTED_DIR.parent());

        // this only works because we do not resolve symlinks when doing traversal
        match self.path.strip_prefix(*REQUESTED_DIR) {
            Ok(_) if self.path.as_ref() == *REQUESTED_DIR => Cow::Borrowed("."),
            Ok(stripped) => stripped.to_string_lossy(),
            Err(_) if Some(self.path.as_ref()) == *REQUESTED_DIR_PARENT => Cow::Borrowed(".."),
            Err(_) => self.path.to_string_lossy(),
        }
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<'_, str> {
        self.display_name()
    }
    fn display(&self, _context: DisplayContext<'_>) -> AnsiString {
        AnsiString::parse(&self.paint_string().to_string())
    }
    fn output(&self) -> Cow<'_, str> {
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
        let vec = vec![PathData::from(selection_candidate)];

        Config::from(vec)
    }
}
