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

use std::collections::BTreeMap;
use std::ops::Deref;

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::config::generate::MountDisplay;
use crate::display_versions::format::QUOTATION_MARKS_LEN;
use crate::MountsForFiles;
use crate::SnapNameMap;
use crate::VersionsMap;

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
        let mut state = serializer.serialize_struct("PrintAsMap", 1)?;

        state.serialize_field("inner", &self.inner)?;
        state.end()
    }
}

impl<'a> From<&MountsForFiles<'a>> for PrintAsMap {
    fn from(mounts_for_files: &MountsForFiles) -> Self {
        let inner = mounts_for_files
            .iter()
            .map(|(key, values)| {
                let res = values
                    .iter()
                    .filter_map(|value| match mounts_for_files.mount_display {
                        MountDisplay::Target => Some(value.path_buf.to_string_lossy().to_string()),
                        MountDisplay::Source => {
                            let opt_md = mounts_for_files
                                .config
                                .dataset_collection
                                .map_of_datasets
                                .inner
                                .get(&value.path_buf);
                            opt_md.map(|md| md.source.clone())
                        }
                        MountDisplay::RelativePath => {
                            let opt_rel_path = key
                                .get_relative_path(
                                    mounts_for_files.config,
                                    value.path_buf.as_path(),
                                )
                                .ok();
                            opt_rel_path.map(|path| path.to_string_lossy().to_string())
                        }
                    })
                    .collect();
                (key.path_buf.to_string_lossy().to_string(), res)
            })
            .collect();
        Self { inner }
    }
}

impl From<&VersionsMap> for PrintAsMap {
    fn from(map: &VersionsMap) -> Self {
        let inner = map
            .iter()
            .map(|(key, values)| {
                let res = values
                    .iter()
                    .map(|value| value.path_buf.to_string_lossy().to_string())
                    .collect();
                (key.path_buf.to_string_lossy().to_string(), res)
            })
            .collect();
        Self { inner }
    }
}

impl From<&SnapNameMap> for PrintAsMap {
    fn from(map: &SnapNameMap) -> Self {
        let inner = map
            .iter()
            .map(|(key, value)| (key.path_buf.to_string_lossy().to_string(), value.clone()))
            .collect();
        Self { inner }
    }
}

impl PrintAsMap {
    pub fn get_map_padding(&self) -> usize {
        self.inner.keys().max_by_key(|key| key.len()).map_or_else(
            || QUOTATION_MARKS_LEN,
            |key| key.len() + QUOTATION_MARKS_LEN,
        )
    }
}
