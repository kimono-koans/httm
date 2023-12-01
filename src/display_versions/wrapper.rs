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

use crate::config::generate::{BulkExclusion, Config, ExecMode, PrintMode};
use crate::data::paths::PathData;
use crate::display_map::format::PrintAsMap;
use crate::library::utility::delimiter;
use crate::lookup::versions::VersionsMap;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::ops::Deref;

pub struct VersionsDisplayWrapper<'a> {
    pub config: &'a Config,
    pub map: VersionsMap,
}

impl<'a> std::string::ToString for VersionsDisplayWrapper<'a> {
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

impl<'a> Deref for VersionsDisplayWrapper<'a> {
    type Target = BTreeMap<PathData, Vec<PathData>>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'a> VersionsDisplayWrapper<'a> {
    pub fn from(config: &'a Config, map: VersionsMap) -> Self {
        Self { config, map }
    }

    pub fn to_json(&self) -> String {
        let res = match self.config.print_mode {
            PrintMode::FormattedNotPretty | PrintMode::RawNewline | PrintMode::RawZero => {
                serde_json::to_string(self)
            }
            PrintMode::FormattedDefault => serde_json::to_string_pretty(self),
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
}

impl<'a> Serialize for VersionsDisplayWrapper<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("VersionMap", 1)?;

        // add live file key to values if needed before serializing
        let new_map: BTreeMap<String, Vec<PathData>> = self
            .deref()
            .clone()
            .into_iter()
            .map(|(key, values)| match &self.config.opt_bulk_exclusion {
                Some(BulkExclusion::NoLive) => (key.path_buf.display().to_string(), values),
                Some(BulkExclusion::NoSnap) => (key.path_buf.display().to_string(), vec![key]),
                None => {
                    let mut new_values = values;
                    new_values.push(key.clone());
                    (key.path_buf.display().to_string(), new_values)
                }
            })
            .collect();

        state.serialize_field("versions", &new_map)?;
        state.end()
    }
}
