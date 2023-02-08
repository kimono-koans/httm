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

use std::borrow::Cow;
use std::ops::Deref;

use terminal_size::{terminal_size, Height, Width};

use crate::config::generate::{BulkExclusion, Config, ExecMode, PrintMode};
use crate::data::paths::{PathData, PHANTOM_DATE, PHANTOM_SIZE};
use crate::library::utility::get_delimiter;
use crate::library::utility::{display_human_size, get_date, paint_string, DateFormat};
use crate::lookup::versions::VersionsMap;

// 2 space wide padding - used between date and size, and size and path
pub const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
pub const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = PRETTY_FIXED_WIDTH_PADDING.len() * 2;
// tab padding used in not so pretty
pub const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
pub const QUOTATION_MARKS_LEN: usize = 2;

impl VersionsMap {
    pub fn format(&self, config: &Config) -> String {
        let global_display_set = DisplaySet::new(config, self);
        let padding_collection = PaddingCollection::new(config, &global_display_set);

        match &config.print_mode {
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty if self.len() == 1 => {
                global_display_set.format(config, &padding_collection)
            }
            _ => self
                .deref()
                .clone()
                .into_iter()
                .map(std::convert::Into::into)
                .map(|raw_instance_set| DisplaySet::new(config, &raw_instance_set))
                .map(|display_set| match config.print_mode {
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        display_set.format(config, &padding_collection)
                    }
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        let delimiter = get_delimiter(config);
                        display_set
                            .iter()
                            .flatten()
                            .map(|pathdata| format!("{}{delimiter}", pathdata.path_buf.display()))
                            .collect()
                    }
                })
                .collect::<String>(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DisplaySet {
    inner: [Vec<PathData>; 2],
}

impl From<[Vec<PathData>; 2]> for DisplaySet {
    fn from(array: [Vec<PathData>; 2]) -> Self {
        Self { inner: array }
    }
}

impl Deref for DisplaySet {
    type Target = [Vec<PathData>; 2];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DisplaySet {
    pub fn new(config: &Config, versions_map: &VersionsMap) -> DisplaySet {
        let vec_snaps = if matches!(config.opt_bulk_exclusion, Some(BulkExclusion::NoSnap)) {
            Vec::new()
        } else {
            versions_map.values().flatten().cloned().collect()
        };

        let vec_live = if config.opt_last_snap.is_some()
            || matches!(config.opt_bulk_exclusion, Some(BulkExclusion::NoLive))
            || matches!(config.exec_mode, ExecMode::MountsForFiles(_))
        {
            Vec::new()
        } else {
            versions_map.keys().cloned().collect()
        };

        Self {
            inner: [vec_snaps, vec_live],
        }
    }

    pub fn format(self, config: &Config, padding_collection: &PaddingCollection) -> String {
        // get the display buffer for each set snaps and live
        self.iter().enumerate().fold(
            String::new(),
            |mut display_set_buffer, (idx, snap_or_live_set)| {
                // a DisplaySet is an array of 2 - idx 0 are the snaps, 1 is the live versions
                let is_snap_set = idx == 0;
                let is_live_set = idx == 1;

                let component_buffer: String = snap_or_live_set
                    .iter()
                    .map(|pathdata| pathdata.format(config, is_live_set, padding_collection))
                    .collect();

                // add each buffer to the set - print fancy border string above, below and between sets
                if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                    display_set_buffer += &component_buffer;
                } else if is_snap_set {
                    display_set_buffer += &padding_collection.fancy_border_string;
                    if !component_buffer.is_empty() {
                        display_set_buffer += &component_buffer;
                        display_set_buffer += &padding_collection.fancy_border_string;
                    }
                } else {
                    display_set_buffer += &component_buffer;
                    display_set_buffer += &padding_collection.fancy_border_string;
                }
                display_set_buffer
            },
        )
    }
}

impl PathData {
    pub fn format(
        &self,
        config: &Config,
        is_live_set: bool,
        padding_collection: &PaddingCollection,
    ) -> String {
        // obtain metadata for timestamp and size
        let metadata = self.get_md_infallible();

        // tab delimited if "no pretty", no border lines, and no colors
        let (display_size, display_path, display_padding) =
            if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                // displays blanks for phantom values, equaling their dummy lens and dates.
                //
                // we use a dummy instead of a None value here.  Basically, sometimes, we want
                // to print the request even if a live file does not exist
                let size = if self.metadata.is_some() {
                    display_human_size(metadata.size)
                } else {
                    padding_collection.phantom_size_pad_str.clone()
                };
                let path = self.path_buf.to_string_lossy();
                let padding = NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
                (size, path, padding)
            // print with padding and pretty border lines and ls colors
            } else {
                let size = {
                    let size = if self.metadata.is_some() {
                        display_human_size(metadata.size)
                    } else {
                        padding_collection.phantom_size_pad_str.clone()
                    };
                    format!(
                        "{:>width$}",
                        size,
                        width = padding_collection.size_padding_len
                    )
                };
                let path = {
                    let path_buf = &self.path_buf;
                    // paint the live strings with ls colors - idx == 1 is 2nd or live set
                    let painted_path_str = if is_live_set {
                        paint_string(self, path_buf.to_str().unwrap_or_default())
                    } else {
                        path_buf.to_string_lossy()
                    };
                    Cow::Owned(format!(
                        "\"{:<width$}\"",
                        painted_path_str,
                        width = padding_collection.size_padding_len
                    ))
                };
                // displays blanks for phantom values, equaling their dummy lens and dates.
                let padding = PRETTY_FIXED_WIDTH_PADDING;
                (size, path, padding)
            };

        let display_date = if self.metadata.is_some() {
            get_date(config, &metadata.modify_time, DateFormat::Display)
        } else {
            padding_collection.phantom_date_pad_str.clone()
        };

        format!(
            "{}{}{}{}{}\n",
            display_date, display_padding, display_size, display_padding, display_path
        )
    }
}

pub struct PaddingCollection {
    pub size_padding_len: usize,
    pub fancy_border_string: String,
    pub phantom_date_pad_str: String,
    pub phantom_size_pad_str: String,
}

impl PaddingCollection {
    pub fn new(config: &Config, display_set: &DisplaySet) -> PaddingCollection {
        // calculate padding and borders for display later
        let (size_padding_len, fancy_border_len) = display_set.iter().flatten().fold(
            (0usize, 0usize),
            |(mut size_padding_len, mut fancy_border_len), pathdata| {
                let metadata = pathdata.get_md_infallible();

                let (display_date, display_size, display_path) = {
                    let date = get_date(config, &metadata.modify_time, DateFormat::Display);
                    let size = format!(
                        "{:>width$}",
                        display_human_size(metadata.size),
                        width = size_padding_len
                    );
                    let path = pathdata.path_buf.to_string_lossy();

                    (date, size, path)
                };

                let display_size_len = display_human_size(metadata.size).len();
                let formatted_line_len = display_date.len()
                    + display_size.len()
                    + display_path.len()
                    + PRETTY_FIXED_WIDTH_PADDING_LEN_X2
                    + QUOTATION_MARKS_LEN;

                size_padding_len = display_size_len.max(size_padding_len);
                fancy_border_len = formatted_line_len.max(fancy_border_len);
                (size_padding_len, fancy_border_len)
            },
        );

        let fancy_border_string: String = Self::get_fancy_border_string(fancy_border_len);

        let phantom_date_pad_str = format!(
            "{:<width$}",
            "",
            width = get_date(config, &PHANTOM_DATE, DateFormat::Display).len()
        );
        let phantom_size_pad_str = format!(
            "{:<width$}",
            "",
            width = display_human_size(PHANTOM_SIZE).len()
        );

        PaddingCollection {
            size_padding_len,
            fancy_border_string,
            phantom_date_pad_str,
            phantom_size_pad_str,
        }
    }

    fn get_fancy_border_string(fancy_border_len: usize) -> String {
        let get_max_sized_border = || {
            // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
            // (0..fancy_border_len).map(|_| "─").collect()
            format!("{:─<fancy_border_len$}\n", "")
        };

        match terminal_size() {
            Some((Width(width), Height(_height))) => {
                if (width as usize) < fancy_border_len {
                    // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
                    // (0..width as usize).map(|_| "─").collect()
                    format!("{:─<width$}\n", "", width = width as usize)
                } else {
                    get_max_sized_border()
                }
            }
            None => get_max_sized_border(),
        }
    }
}
