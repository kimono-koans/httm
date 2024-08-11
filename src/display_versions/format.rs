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

use crate::config::generate::{BulkExclusion, Config, PrintMode};
use crate::data::paths::{PathData, PHANTOM_DATE, PHANTOM_SIZE};
use crate::library::utility::{
    date_string, delimiter, display_human_size, paint_string, DateFormat,
};
use crate::lookup::versions::ProximateDatasetAndOptAlts;
use crate::parse::mounts::IsFilterDir;
use crate::VersionsDisplayWrapper;
use std::borrow::Cow;
use std::ops::Deref;
use terminal_size::{terminal_size, Height, Width};

// 2 space wide padding - used between date and size, and size and path
pub const PRETTY_FIXED_WIDTH_PADDING: &str = "  ";
// our FIXED_WIDTH_PADDING is used twice
pub const PRETTY_FIXED_WIDTH_PADDING_LEN_X2: usize = 4;
// tab padding used in not so pretty
pub const NOT_SO_PRETTY_FIXED_WIDTH_PADDING: &str = "\t";
// and we add 2 quotation marks to the path when we format
pub const QUOTATION_MARKS_LEN: usize = 2;

impl<'a> VersionsDisplayWrapper<'a> {
    pub fn format(&self) -> String {
        let keys: Vec<&PathData> = self.keys().collect();
        let values: Vec<&PathData> = self.values().flatten().collect();

        let global_display_set = DisplaySet::from((keys, values));
        let padding_collection = PaddingCollection::new(self.config, &global_display_set);

        // if a single instance immediately return the global we already prepared
        if matches!(
            self.config.print_mode,
            PrintMode::FormattedDefault | PrintMode::FormattedNotPretty
        ) && self.len() == 1
        {
            return global_display_set.format(self.config, &padding_collection);
        }

        // else re compute for each instance and print per instance, now with uniform padding
        self.iter()
            .map(|(key, values)| {
                let keys: Vec<&PathData> = vec![key];
                let values: Vec<&PathData> = values.iter().collect();

                let display_set = DisplaySet::from((keys, values));

                match &self.config.print_mode {
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        display_set.format(self.config, &padding_collection)
                    }
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        let delimiter = delimiter();

                        display_set
                            .iter()
                            .enumerate()
                            .map(|(idx, snap_or_live_set)| {
                                (DisplaySetType::from(idx), snap_or_live_set)
                            })
                            .filter(|(display_set_type, _snap_or_live_set)| {
                                display_set_type.filter_bulk_exclusions(self.config)
                            })
                            .flat_map(|(_idx, snap_or_live_set)| snap_or_live_set)
                            .fold(String::new(), |mut buffer, pathdata| {
                                buffer.push_str(&pathdata.path().to_string_lossy());
                                buffer.push(delimiter);
                                buffer
                            })
                    }
                }
            })
            .collect::<String>()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DisplaySet<'a> {
    inner: [Vec<&'a PathData>; 2],
}

impl<'a> From<(Vec<&'a PathData>, Vec<&'a PathData>)> for DisplaySet<'a> {
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
            0 => DisplaySetType::IsSnap,
            1 => DisplaySetType::IsLive,
            _ => unreachable!(),
        }
    }
}

impl DisplaySetType {
    #[inline]
    fn filter_bulk_exclusions(&self, config: &Config) -> bool {
        match &self {
            DisplaySetType::IsLive
                if matches!(config.opt_bulk_exclusion, Some(BulkExclusion::NoLive)) =>
            {
                false
            }
            DisplaySetType::IsSnap
                if matches!(config.opt_bulk_exclusion, Some(BulkExclusion::NoSnap)) =>
            {
                false
            }
            _ => true,
        }
    }
}

impl<'a> DisplaySet<'a> {
    pub fn format(&self, config: &Config, padding_collection: &PaddingCollection) -> String {
        let mut border: String = padding_collection.fancy_border_string.to_string();

        // get the display buffer for each set snaps and live
        self.iter()
            .enumerate()
            .map(|(idx, snap_or_live_set)| (DisplaySetType::from(idx), snap_or_live_set))
            .filter(|(display_set_type, _snap_or_live_set)| {
                display_set_type.filter_bulk_exclusions(config)
            })
            .fold(
                String::new(),
                |mut display_set_buffer, (display_set_type, snap_or_live_set)| {
                    let mut component_buffer: String = snap_or_live_set
                        .iter()
                        .map(|pathdata| {
                            pathdata.format(config, &display_set_type, padding_collection)
                        })
                        .collect();

                    // add each buffer to the set - print fancy border string above, below and between sets
                    if matches!(config.print_mode, PrintMode::FormattedNotPretty) {
                        display_set_buffer += &component_buffer;
                    } else if matches!(display_set_type, DisplaySetType::IsSnap) {
                        if component_buffer.is_empty() {
                            let live_pathdata = self.inner[1][0];

                            let warning = live_pathdata.warning_underlying_snaps(config);
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
                    } else {
                        display_set_buffer += &component_buffer;
                        display_set_buffer += &border;
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
        display_set_type: &DisplaySetType,
        padding_collection: &PaddingCollection,
    ) -> String {
        // obtain metadata for timestamp and size
        let metadata = self.metadata_infallible();

        // tab delimited if "no pretty", no border lines, and no colors
        let (display_size, display_path, display_padding) = match &config.print_mode {
            PrintMode::FormattedNotPretty => {
                // displays blanks for phantom values, equaling their dummy lens and dates.
                //
                // we use a dummy instead of a None value here.  Basically, sometimes, we want
                // to print the request even if a live file does not exist
                let size = if self.opt_metadata().is_some() {
                    Cow::Owned(display_human_size(metadata.size()))
                } else {
                    Cow::Borrowed(&padding_collection.phantom_size_pad_str)
                };
                let path = self.path().to_string_lossy();
                let padding = NOT_SO_PRETTY_FIXED_WIDTH_PADDING;
                (size, path, padding)
            }
            _ => {
                // print with padding and pretty border lines and ls colors
                let size = {
                    let size = if self.opt_metadata().is_some() {
                        Cow::Owned(display_human_size(metadata.size()))
                    } else {
                        Cow::Borrowed(&padding_collection.phantom_size_pad_str)
                    };
                    Cow::Owned(format!(
                        "{:>width$}",
                        size,
                        width = padding_collection.size_padding_len
                    ))
                };
                let path = {
                    let path_buf = &self.path();

                    // paint the live strings with ls colors - idx == 1 is 2nd or live set
                    let painted_path_str = match display_set_type {
                        DisplaySetType::IsLive => {
                            paint_string(self, path_buf.to_str().unwrap_or_default())
                        }
                        DisplaySetType::IsSnap => path_buf.to_string_lossy(),
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
        };

        let display_date = if self.opt_metadata().is_some() {
            Cow::Owned(date_string(
                config.requested_utc_offset,
                &metadata.mtime(),
                DateFormat::Display,
            ))
        } else {
            Cow::Borrowed(&padding_collection.phantom_date_pad_str)
        };

        format!(
            "{}{}{}{}{}\n",
            display_date, display_padding, display_size, display_padding, display_path
        )
    }

    fn warning_underlying_snaps<'a>(&'a self, config: &Config) -> &'a str {
        match ProximateDatasetAndOptAlts::new(self).ok() {
            None => {
                "WARN: Could not determine path's most proximate dataset.\n"
            }
            Some(_) if config.opt_omit_ditto => {
                "WARN: Omitting the only snapshot version available, which is identical to the live file.\n"
            }
            Some(_) if self.path().is_filter_dir() => {
                "WARN: Most proximate dataset for path is an unsupported filesystem.\n"
            }
            Some(_) => {
                "WARN: No snapshot version exists for the specified file.\n"
            }
        }
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
                let metadata = pathdata.metadata_infallible();

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
                    let path = pathdata.path().to_string_lossy();

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

        let phantom_date_pad_str = format!(
            "{:<width$}",
            "",
            width = date_string(
                config.requested_utc_offset,
                &PHANTOM_DATE,
                DateFormat::Display
            )
            .chars()
            .count()
        );
        let phantom_size_pad_str = format!(
            "{:<width$}",
            "",
            width = display_human_size(PHANTOM_SIZE).chars().count()
        );

        PaddingCollection {
            size_padding_len,
            fancy_border_string,
            phantom_date_pad_str,
            phantom_size_pad_str,
        }
    }

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
