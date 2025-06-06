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

use crate::GLOBAL_CONFIG;
use crate::config::generate::DedupBy;
use crate::config::generate::{BulkExclusion, Config, FormattedMode, PrintMode, RawMode};
use crate::data::paths::{PHANTOM_DATE, PHANTOM_SIZE, PathData};
use crate::filesystem::mounts::IsFilterDir;
use crate::library::utility::PaintString;
use crate::library::utility::{DateFormat, date_string, display_human_size};
use crate::lookup::versions::ProximateDatasetAndOptAlts;
use nu_ansi_term::AnsiGenericString;
use std::borrow::Cow;
use std::fmt::Debug;
use std::ops::Deref;
use std::sync::LazyLock;
use terminal_size::{Height, Width, terminal_size};
use time::UtcOffset;

// 2 space wide padding - used between date and size, and size and path
pub const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
pub const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = 4;
// tab padding used in not so pretty
pub const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
pub const QUOTATION_MARKS_LEN: usize = 2;

static PHANTOM_DATE_PAD_STR: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{:<width$}",
        "",
        width = date_string(
            GLOBAL_CONFIG.requested_utc_offset,
            &PHANTOM_DATE,
            DateFormat::Display
        )
        .chars()
        .count()
    )
});

static PHANTOM_SIZE_PAD_STR: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{:<width$}",
        "",
        width = display_human_size(PHANTOM_SIZE).chars().count()
    )
});

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DisplaySet<'a> {
    inner: [Vec<&'a PathData>; 2],
}

impl<'a> From<(Vec<&'a PathData>, Vec<&'a PathData>)> for DisplaySet<'a> {
    #[inline(always)]
    fn from((keys, values): (Vec<&'a PathData>, Vec<&'a PathData>)) -> Self {
        Self {
            inner: [values, keys],
        }
    }
}

impl<'a> Deref for DisplaySet<'a> {
    type Target = [Vec<&'a PathData>; 2];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug, Clone)]
pub enum DisplaySetType {
    IsLive,
    IsSnap,
}

impl From<usize> for DisplaySetType {
    #[inline]
    fn from(value: usize) -> Self {
        match value {
            0usize => DisplaySetType::IsSnap,
            1usize => DisplaySetType::IsLive,
            _ => unreachable!(),
        }
    }
}

impl DisplaySetType {
    #[inline(always)]
    pub fn filter_bulk_exclusions(&self, bulk_exclusion: &BulkExclusion) -> bool {
        match &self {
            DisplaySetType::IsLive if matches!(bulk_exclusion, BulkExclusion::NoLive) => false,
            DisplaySetType::IsSnap if matches!(bulk_exclusion, BulkExclusion::NoSnap) => false,
            _ => true,
        }
    }
}

impl<'a> DisplaySet<'a> {
    #[inline(always)]
    pub fn format(&self, config: &Config, padding_collection: &PaddingCollection) -> String {
        let mut border: String = padding_collection.fancy_border_string.clone();

        // get the display buffer for each set snaps and live
        self.iter()
            .enumerate()
            .map(|(idx, snap_or_live_set)| (DisplaySetType::from(idx), snap_or_live_set))
            .filter(|(display_set_type, _snap_or_live_set)| {
                if let Some(bulk_exclusion) = &config.opt_bulk_exclusion {
                    return display_set_type.filter_bulk_exclusions(bulk_exclusion);
                }

                true
            })
            .fold(
                String::new(),
                |mut display_set_buffer, (display_set_type, snap_or_live_set)| {
                    let mut component_buffer: String = snap_or_live_set
                        .iter()
                        .map(|path_data| {
                            path_data.format(config, &display_set_type, padding_collection)
                        })
                        .collect();

                    // add each buffer to the set - print fancy border string above, below and between sets
                    if let PrintMode::Formatted(FormattedMode::NotPretty) = config.print_mode {
                        display_set_buffer += &component_buffer;
                        return display_set_buffer;
                    }

                    match &display_set_type {
                        DisplaySetType::IsSnap => {
                            if component_buffer.is_empty() {
                                let live_path_data = self.inner[1][0];

                                let warning = live_path_data.warn_on_empty_snaps(config);
                                let warning_len = warning.chars().count();
                                let border_len = border.chars().count();

                                if warning_len > border_len {
                                    let diff = warning_len - border_len;
                                    let mut new_border = border.trim_end().to_string();
                                    new_border += &format!("{:─<diff$}\n", "");
                                    border = new_border;
                                }

                                component_buffer = warning.to_string();
                            }

                            display_set_buffer += &border;
                            display_set_buffer += &component_buffer;
                            display_set_buffer += &border;
                        }
                        DisplaySetType::IsLive => {
                            display_set_buffer += &component_buffer;
                            display_set_buffer += &border;
                        }
                    }

                    display_set_buffer
                },
            )
    }

    pub fn into_inner(self) -> [Vec<&'a PathData>; 2] {
        self.inner
    }
}

impl PathData {
    #[inline(always)]
    pub fn format(
        &self,
        config: &Config,
        display_set_type: &DisplaySetType,
        padding_collection: &PaddingCollection,
    ) -> String {
        // obtain metadata for timestamp and size
        let (raw_size, display_date) = match self.opt_metadata() {
            Some(metadata) => {
                let size = Cow::Owned(display_human_size(metadata.size()));

                let display_date = Cow::Owned(date_string(
                    config.requested_utc_offset,
                    &metadata.mtime(),
                    DateFormat::Display,
                ));

                (size, display_date)
            }
            None => (
                Cow::Borrowed(&*PHANTOM_SIZE_PAD_STR),
                Cow::Borrowed(&*PHANTOM_DATE_PAD_STR),
            ),
        };

        // tab delimited if "no pretty", no border lines, and no colors
        let (display_size, display_path, display_padding) = match &config.print_mode {
            PrintMode::Formatted(FormattedMode::NotPretty) => {
                // displays blanks for phantom values, equaling their dummy lens and dates.
                //
                // we use a dummy instead of a None value here.  Basically, sometimes, we want
                // to print the request even if a live file does not exist
                let path = self.path().to_string_lossy();
                let padding = NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
                (raw_size, path, padding)
            }
            PrintMode::Formatted(FormattedMode::Default) => {
                // print with padding and pretty border lines and ls colors
                let size = Cow::Owned(format!(
                    "{:>width$}",
                    raw_size,
                    width = padding_collection.size_padding_len
                ));
                let path = {
                    // paint the live strings with ls colors - idx == 1 is 2nd or live set
                    let painted_path_str = match display_set_type {
                        DisplaySetType::IsLive => self.paint_string(),
                        DisplaySetType::IsSnap => {
                            let path_buf = &self.path();
                            AnsiGenericString::from(path_buf.to_string_lossy())
                        }
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
            }
            _ => unreachable!(),
        };

        format!(
            "{}{}{}{}{}\n",
            display_date, display_padding, display_size, display_padding, display_path
        )
    }

    fn warn_on_empty_snaps(&self, config: &Config) -> &str {
        match ProximateDatasetAndOptAlts::new(self).ok() {
            None => "WARN: Could not determine live path's most proximate dataset.\n",
            _ if self.path().ancestors().any(|mount| mount.is_filter_dir()) => {
                "WARN: Most proximate dataset for path is an unsupported filesystem.\n"
            }
            _ if self.opt_metadata().is_none() => "WARN: Input file may have never existed.\n",
            Some(prox_opt_alts)
                if config.opt_omit_ditto
                    && prox_opt_alts
                        .into_search_bundles()
                        .flat_map(|relative_path_and_snap_mounts| {
                            relative_path_and_snap_mounts.version_search(&DedupBy::Disable)
                        })
                        .count()
                        != 0 =>
            {
                "WARN: Omit ditto enabled.  Omitting the only snapshot version available.\n"
            }
            _ => "WARN: No snapshot version exists for the specified file.\n",
        }
    }

    #[inline(always)]
    pub fn raw_format(
        &self,
        raw_mode: &RawMode,
        delimiter: char,
        requested_utc_offset: UtcOffset,
    ) -> String {
        match raw_mode {
            RawMode::Csv => match self.opt_metadata() {
                Some(md) => {
                    let date =
                        date_string(requested_utc_offset, &md.mtime(), DateFormat::Timestamp);

                    let size = md.size();

                    format!(
                        "{},{},\"{}\"{}",
                        date,
                        size,
                        self.path().to_string_lossy(),
                        delimiter
                    )
                }
                None => {
                    format!(",,\"{}\"{}", self.path().to_string_lossy(), delimiter)
                }
            },
            RawMode::Newline | RawMode::Zero => {
                format!("{}{}", self.path().to_string_lossy(), delimiter)
            }
        }
    }
}

pub struct PaddingCollection {
    pub size_padding_len: usize,
    pub fancy_border_string: String,
}

impl PaddingCollection {
    #[inline(always)]
    pub fn new(config: &Config, display_set: &DisplaySet) -> PaddingCollection {
        // calculate padding and borders for display later
        let (size_padding_len, fancy_border_len) = display_set.iter().flatten().fold(
            (0usize, 0usize),
            |(mut size_padding_len, mut fancy_border_len), path_data| {
                let metadata = path_data.metadata_infallible();

                let (display_date, display_size, display_path) = {
                    let date = date_string(
                        config.requested_utc_offset,
                        &metadata.mtime(),
                        DateFormat::Display,
                    );
                    let size = format!(
                        "{:>width$}",
                        display_human_size(metadata.size()),
                        width = size_padding_len
                    );
                    let path = path_data.path().to_string_lossy();

                    (date, size, path)
                };

                let display_size_len = display_human_size(metadata.size()).chars().count();
                let formatted_line_len = display_date.chars().count()
                    + display_size.chars().count()
                    + display_path.chars().count()
                    + PRETTY_FIXED_WIDTH_PADDING_LEN_X2
                    + QUOTATION_MARKS_LEN;

                size_padding_len = display_size_len.max(size_padding_len);
                fancy_border_len = formatted_line_len.max(fancy_border_len);
                (size_padding_len, fancy_border_len)
            },
        );

        let fancy_border_string: String = Self::fancy_border_string(fancy_border_len);

        PaddingCollection {
            size_padding_len,
            fancy_border_string,
        }
    }

    #[inline(always)]
    fn fancy_border_string(fancy_border_len: usize) -> String {
        if let Some((Width(width), Height(_height))) = terminal_size() {
            let width_as_usize = width as usize;

            if width_as_usize < fancy_border_len {
                // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
                // (0..width as usize).map(|_| "─").collect()
                return format!("{:─<width_as_usize$}\n", "");
            }
        }

        // Active below is the most idiomatic Rust, but it maybe slower than the commented portion
        // (0..fancy_border_len).map(|_| "─").collect()
        // this is the max sized border
        format!("{:─<fancy_border_len$}\n", "")
    }
}
