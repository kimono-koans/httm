use std::ops::Deref;
use std::path::PathBuf;

use hashbrown::HashMap;

use crate::data::paths::PathData;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::HttmResult;
use crate::library::utility::deconstruct_snap_paths;
use crate::parse::aliases::FilesystemType;
use crate::GLOBAL_CONFIG;

pub struct GroupRollback;

impl GroupRollback {
    pub fn new(opt_filter_name: &Option<String>) -> HttmResult<()> {
        let group_rollback_map: HashMap<String, Vec<PathBuf>> = GLOBAL_CONFIG
            .dataset_collection
            .map_of_snaps
            .deref()
            .iter()
            .map(|(key, values)| {
                let parsed_values: Vec<String> = values
                    .iter()
                    .filter_map(|path| {
                        deconstruct_snap_paths(&PathData::from(&path)).and_then(|snap_string| {
                            snap_string
                                .split_once('@')
                                .map(|(_lhs, rhs)| rhs.to_string())
                        })
                    })
                    .filter(|string| {
                        if let Some(filter_name) = opt_filter_name {
                            if string.contains(filter_name) {
                                return false;
                            }
                        }

                        true
                    })
                    .collect();

                (key.clone(), parsed_values)
            })
            .filter_map(|(key, values)| {
                let md = GLOBAL_CONFIG.dataset_collection.map_of_datasets.get(&key);

                md.and_then(|md| {
                    if !matches!(md.fs_type, FilesystemType::Zfs) {
                        return None;
                    }

                    let key_iter = std::iter::repeat_with(|| md.source.clone());

                    let res: Vec<(String, PathBuf)> = values.into_iter().zip(key_iter).collect();
                    Some(res)
                })
            })
            .flatten()
            .into_group_map();

        println!("{:?}", group_rollback_map);

        Ok(())
    }
}
