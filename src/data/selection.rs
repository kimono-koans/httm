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
// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{fs::FileType, path::PathBuf};

use lscolors::Colorable;
use skim::prelude::*;

use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::display_versions::wrapper::VersionsDisplayWrapper;
use crate::library::results::HttmResult;
use crate::library::utility::paint_string;
use crate::{VersionsMap, GLOBAL_CONFIG};

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    path: PathBuf,
    file_type: Option<FileType>,
}

impl SelectionCandidate {
    pub fn new(basic_info: BasicDirEntryInfo, is_phantom: bool) -> Self {
        SelectionCandidate {
            path: basic_info.path,
            // here save space of bool/padding instead of an "is_phantom: bool"
            //
            // issue: conflate not having a file_type as phantom
            // for purposes of coloring the file_name/path only?
            //
            // std lib docs don't give much indication as to
            // when file_type() fails?  Doesn't seem to be a problem?
            file_type: {
                if is_phantom {
                    None
                } else {
                    basic_info.file_type
                }
            },
        }
    }

    fn preview_view(&self) -> HttmResult<String> {
        let config = &GLOBAL_CONFIG;
        let paths_selected = &[PathData::from(self.path.as_path())];

        // generate a config for display
        let display_config = config.generate_display_config(paths_selected);

        // finally run search on those paths
        let versions_map = VersionsMap::new(&display_config, &display_config.paths)?;
        let output_buf = VersionsDisplayWrapper::from(&display_config, versions_map).to_string();

        Ok(output_buf)
    }

    fn generate_display_name(&self) -> Cow<str> {
        self.path
            .strip_prefix(
                &GLOBAL_CONFIG
                    .opt_requested_dir
                    .as_ref()
                    .expect("requested_dir should never be None in Interactive Browse mode")
                    .path_buf,
            )
            .unwrap_or(&self.path)
            .to_string_lossy()
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
        AnsiString::parse(&paint_string(self, &self.generate_display_name()))
    }
    fn output(&self) -> Cow<str> {
        self.text()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let preview_output = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(preview_output)
    }
}
