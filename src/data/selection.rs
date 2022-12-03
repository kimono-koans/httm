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

use crate::config::generate::{Config, ExecMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::library::results::HttmResult;
use crate::library::utility::paint_string;
use crate::lookup::versions::versions_lookup_exec;

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    config: Arc<Config>,
    path: PathBuf,
    file_type: Option<FileType>,
}

impl SelectionCandidate {
    pub fn new(config: Arc<Config>, basic_info: BasicDirEntryInfo, is_phantom: bool) -> Self {
        SelectionCandidate {
            config,
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

    // use an associated function her because we may need this display again elsewhere
    pub fn generate_config_for_display(config: &Config, paths_selected: &[PathData]) -> Config {
        // generate a config for a preview display only
        Config {
            paths: paths_selected.to_vec(),
            opt_raw: false,
            opt_zeros: false,
            opt_no_pretty: false,
            opt_recursive: false,
            opt_no_live: false,
            opt_exact: false,
            opt_overwrite: false,
            opt_no_filter: false,
            opt_no_snap: false,
            opt_debug: false,
            opt_no_traverse: false,
            opt_no_hidden: false,
            opt_last_snap: None,
            opt_preview: None,
            opt_omit_ditto: config.opt_omit_ditto,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::Display,
            deleted_mode: None,
            dataset_collection: config.dataset_collection.clone(),
            pwd: config.pwd.clone(),
            opt_requested_dir: config.opt_requested_dir.clone(),
        }
    }

    fn preview_view(&self) -> HttmResult<String> {
        let config = &self.config;
        let paths_selected = &[PathData::from(self.path.as_path())];

        // generate a config for display
        let gen_config = SelectionCandidate::generate_config_for_display(config, paths_selected);

        // finally run search on those paths
        let map_live_to_snaps = versions_lookup_exec(&gen_config, &gen_config.paths)?;
        // and display
        let output_buf = map_live_to_snaps.display(&gen_config);

        Ok(output_buf)
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
    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&paint_string(
            self,
            &self
                .path
                .strip_prefix(
                    &self
                        .config
                        .opt_requested_dir
                        .as_ref()
                        .expect("requested_dir should never be None in Interactive Browse mode")
                        .path_buf,
                )
                .unwrap_or(&self.path)
                .to_string_lossy(),
        ))
    }
    fn output(&self) -> Cow<str> {
        self.text()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let preview_output = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(preview_output)
    }
}
