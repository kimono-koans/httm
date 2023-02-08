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

use std::{
    ffi::OsString,
    fs::read_dir,
    ops::Deref,
    path::{Path, PathBuf},
};

use hashbrown::{HashMap, HashSet};

use crate::config::generate::Config;
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::library::results::HttmResult;
use crate::lookup::versions::{MostProximateAndOptAlts, RelativePathAndSnapMounts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFilesBundle {
    inner: Vec<BasicDirEntryInfo>,
}

impl From<Vec<BasicDirEntryInfo>> for DeletedFilesBundle {
    fn from(vec: Vec<BasicDirEntryInfo>) -> Self {
        Self { inner: vec }
    }
}

impl Deref for DeletedFilesBundle {
    type Target = Vec<BasicDirEntryInfo>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DeletedFilesBundle {
    pub fn into_inner(self) -> Vec<BasicDirEntryInfo> {
        self.inner
    }
}

// deleted_lookup_exec is a dumb function/module if we want to rank outputs, get last in time, etc.
// we do that elsewhere.  deleted is simply about finding at least one version of a deleted file
// this, believe it or not, will be faster
impl DeletedFilesBundle {
    pub fn new(config: &Config, requested_dir: &Path) -> Self {
        // we always need a requesting dir because we are comparing the files in the
        // requesting dir to those of their relative dirs on snapshots
        let requested_dir_pathdata = PathData::from(requested_dir);

        let requested_snap_datasets = config
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        // create vec of all local and replicated backups at once
        //
        // we need to make certain that what we return from possibly multiple datasets are unique
        // as these will be the filenames that populate our interactive views, so deduplicate
        // by filename and latest file version here
        let basic_info_map: HashMap<OsString, BasicDirEntryInfo> = requested_snap_datasets
            .iter()
            .flat_map(|dataset_type| {
                MostProximateAndOptAlts::new(config, &requested_dir_pathdata, dataset_type)
            })
            .flat_map(|datasets_of_interest| {
                MostProximateAndOptAlts::get_search_bundles(
                    config,
                    datasets_of_interest,
                    &requested_dir_pathdata,
                )
            })
            .flatten()
            .flat_map(|search_bundle| {
                Self::get_unique_deleted_for_dir(&requested_dir_pathdata.path_buf, &search_bundle)
            })
            .flatten()
            .map(|basic_info| (basic_info.file_name.clone(), basic_info))
            .collect();

        let inner = basic_info_map.into_values().collect();

        DeletedFilesBundle { inner }
    }

    fn get_unique_deleted_for_dir(
        requested_dir: &Path,
        search_bundle: &RelativePathAndSnapMounts,
    ) -> HttmResult<Vec<BasicDirEntryInfo>> {
        // get all local entries we need to compare against these to know
        // what is a deleted file
        //
        // create a collection of local file names
        let local_filenames_set: HashSet<OsString> = read_dir(requested_dir)?
            .flatten()
            .map(|dir_entry| dir_entry.file_name())
            .collect();

        let unique_snap_filenames: HashMap<OsString, BasicDirEntryInfo> =
            Self::get_unique_snap_filenames(search_bundle.snap_mounts, search_bundle.relative_path);

        // compare local filenames to all unique snap filenames - none values are unique, here
        let all_deleted_versions: Vec<BasicDirEntryInfo> = unique_snap_filenames
            .into_iter()
            .filter_map(|(file_name, basic_info)| {
                if !local_filenames_set.contains(&file_name) {
                    Some(basic_info)
                } else {
                    None
                }
            })
            .collect();

        Ok(all_deleted_versions)
    }

    fn get_unique_snap_filenames(
        mounts: &[PathBuf],
        relative_path: &Path,
    ) -> HashMap<OsString, BasicDirEntryInfo> {
        mounts
            .iter()
            .map(|path| path.join(relative_path))
            .flat_map(read_dir)
            .flatten()
            .flatten()
            .map(|dir_entry| (dir_entry.file_name(), BasicDirEntryInfo::from(&dir_entry)))
            .collect::<HashMap<OsString, BasicDirEntryInfo>>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastInTimeSet {
    inner: Vec<PathBuf>,
}

impl Deref for LastInTimeSet {
    type Target = Vec<PathBuf>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl LastInTimeSet {
    // this is very similar to VersionsMap, but of course returns only last in time
    // for directory paths during deleted searches.  it's important to have a policy, here,
    // last in time, for which directory we return during deleted searches, because
    // different snapshot-ed dirs may contain different files.

    // this fn is also missing parallel iter fns, to make the searches more responsive
    // by leaving parallel search for the interactive views
    pub fn new(config: &Config, path_set: &[PathData]) -> Self {
        // create vec of all local and replicated backups at once
        let snaps_selected_for_search = config
            .dataset_collection
            .snaps_selected_for_search
            .get_value();

        let inner: Vec<PathBuf> = path_set
            .iter()
            .filter_map(|pathdata| {
                snaps_selected_for_search
                    .iter()
                    .flat_map(|dataset_type| {
                        MostProximateAndOptAlts::new(config, pathdata, dataset_type)
                    })
                    .flat_map(|datasets_of_interest| {
                        MostProximateAndOptAlts::get_search_bundles(
                            config,
                            datasets_of_interest,
                            pathdata,
                        )
                    })
                    .flatten()
                    .flat_map(|search_bundle| search_bundle.get_last_version())
                    .filter(|pathdata| pathdata.metadata.is_some())
                    .max_by_key(|pathdata| pathdata.metadata.unwrap().modify_time)
                    .map(|pathdata| pathdata.path_buf)
            })
            .collect();

        Self { inner }
    }
}
