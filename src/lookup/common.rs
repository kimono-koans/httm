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

// use crate::config::generate::Config;
// use crate::data::paths::PathData;
// use crate::library::results::HttmResult;
// use crate::lookup::versions::MostProximateAndOptAlts;
// use crate::lookup::versions::RelativePathAndSnapMounts;
// use crate::lookup::versions::SnapDatasetType;

// #[derive(Debug, Clone, PartialEq, Eq)]
// pub struct FindVersions;

// impl FindVersions {
//     pub fn exec<'b>(
//         config: &'b Config,
//         pathdata: &'b PathData,
//         dataset_type: &'b SnapDatasetType,
//     ) -> HttmResult<Vec<RelativePathAndSnapMounts<'b>>> {
//         let datasets_of_interest = MostProximateAndOptAlts::new(config, pathdata, dataset_type)?;
//         let vec_search_bundle = datasets_of_interest.get_search_bundles(config, pathdata)?;

//         Ok(vec_search_bundle.to_owned())
//     }
// }
