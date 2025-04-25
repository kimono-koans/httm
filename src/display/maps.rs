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

use crate::config::generate::{FormattedMode, PrintMode, RawMode};
use crate::data::paths::{PathData, ZfsSnapPathGuard};
use crate::display::versions::{NOT_SO_PRETTY_FIXED_WIDTH_PADDING, QUOTATION_MARKS_LEN};
use crate::library::utility::delimiter;
use crate::{GLOBAL_CONFIG, MountsForFiles, SnapNameMap, VersionsMap};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::ops::Deref;

#[derive(Debug)]
pub struct PrintAsMap {
    inner: BTreeMap<String, Vec<String>>,
}

impl Deref for PrintAsMap {
    type Target = BTreeMap<String, Vec<String>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<BTreeMap<String, Vec<String>>> for PrintAsMap {
    fn from(map: BTreeMap<String, Vec<String>>) -> Self {
        Self { inner: map }
    }
}

impl Serialize for PrintAsMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_map(Some(self.inner.len()))?;
        self.inner
            .iter()
            .try_for_each(|(k, v)| state.serialize_entry(k, v))?;
        state.end()
    }
}

impl<'a> From<&MountsForFiles<'a>> for PrintAsMap {
    fn from(mounts_for_files: &MountsForFiles) -> Self {
        let mount_display = mounts_for_files.mount_display();

        let inner = mounts_for_files
            .iter()
            .map(|prox| {
                let path_data = prox.path_data();

                let res = prox
                    .datasets_of_interest()
                    .map(PathData::from)
                    .filter_map(|mount| match &ZfsSnapPathGuard::new(path_data) {
                        Some(spg) => mount_display.display(spg, &mount),
                        None => mount_display.display(path_data, &mount),
                    })
                    .map(|path| path.to_string_lossy().to_string())
                    .collect();

                (path_data.path().to_string_lossy().to_string(), res)
            })
            .collect();
        Self { inner }
    }
}

impl<'a> From<&'a VersionsMap<'a>> for PrintAsMap {
    fn from(map: &VersionsMap) -> Self {
        let inner = map
            .iter()
            .map(|(key, values)| {
                let res = values
                    .iter()
                    .map(|value| value.path().to_string_lossy().to_string())
                    .collect();
                (key.path_data().path().to_string_lossy().to_string(), res)
            })
            .collect();
        Self { inner }
    }
}

impl From<&SnapNameMap> for PrintAsMap {
    fn from(map: &SnapNameMap) -> Self {
        let inner = map
            .iter()
            .map(|(key, value)| (key.path().to_string_lossy().to_string(), value.clone()))
            .collect();
        Self { inner }
    }
}

impl std::string::ToString for PrintAsMap {
    fn to_string(&self) -> String {
        if GLOBAL_CONFIG.opt_json {
            return self.to_json();
        }

        match &GLOBAL_CONFIG.print_mode {
            PrintMode::Raw(_) => {
                let delimiter = if let PrintMode::Raw(RawMode::Csv) = GLOBAL_CONFIG.print_mode {
                    ','
                } else {
                    delimiter()
                };

                let last = self.values().len() - 1;

                self.values().flatten().enumerate().fold(
                    String::new(),
                    |mut buffer, (idx, value)| {
                        buffer.push_str(value);

                        if let PrintMode::Raw(RawMode::Csv) = GLOBAL_CONFIG.print_mode {
                            if last == idx {
                                buffer.push('\n');
                                return buffer;
                            }
                        }

                        buffer.push(delimiter);
                        buffer
                    },
                )
            }
            PrintMode::Formatted(_) => self.format(),
        }
    }
}

impl PrintAsMap {
    pub fn map_padding(&self) -> usize {
        self.keys().max_by_key(|key| key.len()).map_or_else(
            || QUOTATION_MARKS_LEN,
            |key| key.len() + QUOTATION_MARKS_LEN,
        )
    }

    pub fn to_json(&self) -> String {
        let res = match GLOBAL_CONFIG.print_mode {
            PrintMode::Formatted(FormattedMode::Default) => serde_json::to_string_pretty(&self),
            _ => serde_json::to_string(&self),
        };

        match res {
            Ok(s) => {
                let delimiter = delimiter();
                format!("{s}{delimiter}")
            }
            Err(error) => {
                eprintln!("Error: {error}");
                std::process::exit(1)
            }
        }
    }

    pub fn format(&self) -> String {
        let padding = self.map_padding();

        let write_out_buffer = self
            .iter()
            .filter(|(_key, values)| {
                if GLOBAL_CONFIG.opt_last_snap.is_some() {
                    !values.is_empty()
                } else {
                    true
                }
            })
            .map(|(key, values)| {
                let display_path = if matches!(
                    &GLOBAL_CONFIG.print_mode,
                    PrintMode::Formatted(FormattedMode::NotPretty)
                ) {
                    key.clone()
                } else {
                    format!("\"{key}\"")
                };

                let values_string: String = values
                    .iter()
                    .enumerate()
                    .map(|(idx, value)| {
                        if matches!(
                            &GLOBAL_CONFIG.print_mode,
                            PrintMode::Formatted(FormattedMode::NotPretty)
                        ) {
                            format!("{NOT_SO_PRETTY_FIXED_WIDTH_PADDING}{value}")
                        } else if idx == 0 {
                            format!(
                                "{:<width$} : \"{}\"\n",
                                display_path,
                                value,
                                width = padding
                            )
                        } else {
                            format!("{:<padding$} : \"{value}\"\n", "")
                        }
                    })
                    .collect::<String>();

                if matches!(
                    &GLOBAL_CONFIG.print_mode,
                    PrintMode::Formatted(FormattedMode::NotPretty)
                ) {
                    format!("{display_path}:{values_string}\n")
                } else {
                    values_string
                }
            })
            .collect();

        write_out_buffer
    }
}
