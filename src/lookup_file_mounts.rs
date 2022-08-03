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

use itertools::Itertools;
use rayon::prelude::*;

use crate::lookup_versions::get_snap_dataset_for_search;
use crate::utility::PathData;
use crate::{Config, ExecMode, HttmResult, SnapDatasetType, SnapDatasetsBundle};

pub type MountsForFiles = BTreeMap<PathData, Vec<PathData>>;

#[allow(clippy::type_complexity)]
pub fn get_mounts_for_files(config: &Config) -> HttmResult<MountsForFiles> {
    // we only check for phantom files in "mount for file" mode because
    // people should be able to search for deleted files in other modes
    let (non_phantom_files, phantom_files): (Vec<&PathData>, Vec<&PathData>) = config
        .paths
        .par_iter()
        .partition(|pathdata| pathdata.metadata.is_some());

    if !phantom_files.is_empty() {
        eprintln!(
            "httm was unable to determine mount locations for all input files, \
        because the following files do not appear to exist: "
        );

        phantom_files
            .iter()
            .for_each(|pathdata| eprintln!("{}", pathdata.path_buf.to_string_lossy()));
    }

    // don't want to request alt replicated mounts in snap mode, though we may in opt_mount_for_file mode
    let selected_datasets = if config.exec_mode == ExecMode::SnapFileMount {
        vec![SnapDatasetType::MostProximate]
    } else {
        config.dataset_collection.snaps_for_search.clone()
    };

    let mounts_for_files: MountsForFiles = non_phantom_files
        .into_iter()
        .map(|pathdata| {
            let datasets: Vec<SnapDatasetsBundle> = selected_datasets
                .iter()
                .flat_map(|dataset_type| {
                    get_snap_dataset_for_search(config, pathdata, dataset_type)
                })
                .collect();
            (pathdata.clone(), datasets)
        })
        .into_group_map_by(|(pathdata, _snap_types_for_search)| pathdata.clone())
        .into_iter()
        .map(|(pathdata, vec_snap_types_for_search)| {
            let datasets: Vec<PathData> = vec_snap_types_for_search
                .into_iter()
                .flat_map(|(_proximate_mount, snap_types_for_search)| snap_types_for_search)
                .flat_map(|snap_types_for_search| snap_types_for_search.get_datasets_of_interest())
                .map(|path| PathData::from(path.as_path()))
                .rev()
                .collect();
            (pathdata, datasets)
        })
        .collect();

    Ok(mounts_for_files)
}
