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
    BulkExclusion,
    Config,
    ExecMode,
    FormattedMode,
    NumVersionsMode,
    PrintMode,
    RawMode,
};
use crate::data::paths::PathData;
use crate::display::maps::PrintAsMap;
use crate::display::versions::{
    DisplaySet,
    DisplaySetType,
    PaddingCollection,
};
use crate::library::utility::delimiter;
use crate::lookup::versions::{
    Versions,
    VersionsMap,
};
use crate::{
    GLOBAL_CONFIG,
    exit_error,
};
use hashbrown::HashMap;
use serde::ser::SerializeMap;
use serde::{
    Serialize,
    Serializer,
};
use std::ops::Deref;

pub struct DisplayWrapper<'a> {
    config: &'a Config,
    map: VersionsMap,
}

impl<'a> DisplayWrapper<'a> {
    pub fn format(&self) -> String {
        // if a single instance immediately return the global we already prepared
        match &self.config.print_mode {
            PrintMode::Formatted(_) => {
                let keys: Vec<&PathData> = self.keys().collect();
                let values: Vec<&PathData> = self.values().flatten().collect();

                let global_display_set = DisplaySet::from((keys, values));
                let padding_collection = PaddingCollection::new(self.config, &global_display_set);

                if self.len() == 1 {
                    return global_display_set.format(self.config, &padding_collection);
                }

                // else re compute for each instance and print per instance, now with uniform padding
                self.iter()
                    .map(|(key, values)| {
                        let keys: Vec<&PathData> = vec![key];
                        let values: Vec<&PathData> = values.iter().collect();

                        let display_set = DisplaySet::from((keys, values));

                        display_set.format(self.config, &padding_collection)
                    })
                    .collect::<String>()
            }
            PrintMode::Raw(raw_mode) => {
                let delimiter: char = delimiter();

                // else re compute for each instance and print per instance, now with uniform padding
                self.iter()
                    .map(|(key, values)| {
                        let keys: Vec<&PathData> = vec![key];
                        let values: Vec<&PathData> = values.iter().collect();

                        (keys, values)
                    })
                    .map(|(keys, values)| DisplaySet::from((keys, values)))
                    .map(|display_set| {
                        display_set
                            .into_inner()
                            .into_iter()
                            .enumerate()
                            .filter(|(idx, _set)| {
                                let display_set_type = DisplaySetType::from(*idx);

                                if let Some(bulk_exclusion) = &self.config.opt_bulk_exclusion {
                                    return display_set_type.filter_bulk_exclusions(bulk_exclusion);
                                }

                                true
                            })
                            .map(|(_idx, set)| set)
                            .flatten()
                    })
                    .flatten()
                    .map(|pd| pd.raw_format(raw_mode, delimiter, self.config.requested_utc_offset))
                    .collect::<String>()
            }
        }
    }

    pub fn format_as_num_versions(&self, num_versions_mode: &NumVersionsMode) -> String {
        // let delimiter = get_delimiter(config);
        let delimiter = delimiter();

        let printable_map = PrintAsMap::from(&self.map);

        let map_padding = printable_map.map_padding();

        let total_num_paths = self.len();

        let print_mode = &GLOBAL_CONFIG.print_mode;

        let write_out_buffer: String = self
            .map
            .deref()
            .into_iter()
            .map(|(live_version, snaps)| Versions::from_raw(live_version.clone(), snaps.clone()))
            .filter_map(|versions| {
                Self::parse_num_versions(
                    num_versions_mode,
                    print_mode,
                    delimiter,
                    &versions,
                    map_padding,
                    total_num_paths,
                )
            })
            .collect();

        if write_out_buffer.is_empty() {
            let description = match num_versions_mode {
                NumVersionsMode::Multiple => {
                    "Notification: No paths which have multiple versions exist."
                }
                NumVersionsMode::SingleAll
                | NumVersionsMode::SingleNoSnap
                | NumVersionsMode::SingleWithSnap => {
                    "Notification: No paths which have only a single version exist."
                }
                // NumVersionsMode::All empty should be dealt with earlier at lookup_exec
                NumVersionsMode::AllNumerals | NumVersionsMode::AllGraph => unreachable!(),
            };
            eprintln!("{description}");
        }

        write_out_buffer
    }

    fn parse_num_versions(
        num_versions_mode: &NumVersionsMode,
        print_mode: &PrintMode,
        delimiter: char,
        versions: &Versions,
        padding: usize,
        total_num_paths: usize,
    ) -> Option<String> {
        let display_path = versions.live_path_data().path().display();

        let mut num_versions = versions.snap_versions().len();

        match num_versions_mode {
            NumVersionsMode::AllGraph => {
                if !versions.is_live_version_redundant() {
                    num_versions += 1
                };

                match print_mode {
                    PrintMode::Formatted(FormattedMode::Default) => Some(format!(
                        "{:<width$} : {:*<num_versions$}{}",
                        display_path,
                        "",
                        delimiter,
                        width = padding
                    )),
                    _ => {
                        unreachable!()
                    }
                }
            }
            NumVersionsMode::AllNumerals => {
                if !versions.is_live_version_redundant() {
                    num_versions += 1
                };

                match print_mode {
                    PrintMode::Formatted(FormattedMode::Default) => Some(format!(
                        "{:<width$} : {}{}",
                        display_path,
                        num_versions,
                        delimiter,
                        width = padding
                    )),
                    PrintMode::Raw(RawMode::Csv) => {
                        Some(format!("{},{num_versions}{}", display_path, delimiter))
                    }
                    PrintMode::Raw(_) if total_num_paths == 1 => {
                        Some(format!("{num_versions}{}", delimiter))
                    }
                    PrintMode::Formatted(FormattedMode::NotPretty) | _ => {
                        Some(format!("{}\t{num_versions}{}", display_path, delimiter))
                    }
                }
            }
            NumVersionsMode::Multiple => {
                if num_versions == 0 || (num_versions == 1 && versions.is_live_version_redundant())
                {
                    None
                } else {
                    Some(format!("{display_path}{delimiter}"))
                }
            }
            NumVersionsMode::SingleAll => {
                if num_versions == 0 || (num_versions == 1 && versions.is_live_version_redundant())
                {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
            NumVersionsMode::SingleNoSnap => {
                if num_versions == 0 {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
            NumVersionsMode::SingleWithSnap => {
                if num_versions == 1 && versions.is_live_version_redundant() {
                    Some(format!("{display_path}{delimiter}"))
                } else {
                    None
                }
            }
        }
    }
}

impl<'a> std::string::ToString for DisplayWrapper<'a> {
    fn to_string(&self) -> String {
        match &self.config.exec_mode {
            ExecMode::NumVersions(num_versions_mode) => {
                self.format_as_num_versions(num_versions_mode)
            }
            _ => {
                if self.config.opt_last_snap.is_some() {
                    let printable_map = PrintAsMap::from(&self.map);
                    return printable_map.to_string();
                }

                if self.config.opt_json {
                    return self.to_json();
                }

                self.format()
            }
        }
    }
}

impl<'a> Deref for DisplayWrapper<'a> {
    type Target = HashMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'a> DisplayWrapper<'a> {
    pub fn from(config: &'a Config, map: VersionsMap) -> Self {
        Self { config, map }
    }

    pub fn to_json(&self) -> String {
        let res = match self.config.print_mode {
            PrintMode::Formatted(FormattedMode::Default) => serde_json::to_string_pretty(self),
            _ => serde_json::to_string(self),
        };

        match res {
            Ok(s) => {
                let delimiter = delimiter();
                format!("{s}{delimiter}")
            }
            Err(error) => {
                exit_error(error.into());
                unreachable!();
            }
        }
    }
}

impl<'a> Serialize for DisplayWrapper<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // add live file key to values if needed before serializing
        let new_map: HashMap<String, Vec<PathData>> = self
            .deref()
            .clone()
            .into_iter()
            .map(|(key, values)| match &self.config.opt_bulk_exclusion {
                Some(BulkExclusion::NoLive) => (key.path().display().to_string(), values),
                Some(BulkExclusion::NoSnap) => (key.path().display().to_string(), vec![key]),
                None => {
                    let mut new_values = values;
                    new_values.push(key.clone());
                    (key.path().display().to_string(), new_values)
                }
            })
            .collect();

        let mut state = serializer.serialize_map(Some(new_map.len()))?;
        new_map
            .iter()
            .try_for_each(|(k, v)| state.serialize_entry(k, v))?;
        state.end()
    }
}
