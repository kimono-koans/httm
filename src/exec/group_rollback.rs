use std::ops::Deref;

use crate::GLOBAL_CONFIG;
use crate::library::results::HttmResult;
use crate::lookup::snap_names::SnapNameMap;

pub struct GroupRollback;

impl GroupRollback {
    pub fn new(filter_name: &Option<String>) -> HttmResult<()> {
        let group_rollback_map = GLOBAL_CONFIG.dataset_collection.map_of_snaps
            .deref()
            .iter()
            .map(|(key, value)| {
                SnapNameMap::new()
            })


        Ok(())
    }
}
