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

#[macro_use]
extern crate lazy_static;
extern crate proc_mounts;

use std::{
    fs::canonicalize,
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

// so we may use AHashMap with Parallel Iterators!
// impl of AHashMap as (K,V) instead of (K,V,S) so we
// may use with collect() function, see rayon::FromParallelIterator
use ahash::AHasher;
pub type AHashBuildHasher = BuildHasherDefault<AHasher>;
pub type AHashMap<K, V> = std::collections::HashMap<K, V, AHashBuildHasher>;
use AHashMap as HashMap;

use clap::{crate_name, crate_version, Arg, ArgMatches};
use lookup_versions::DatasetsForSearch;
use rayon::prelude::*;
use utility::print_output_buf;

mod display;
mod interactive;
mod lookup_deleted;
mod lookup_versions;
mod parse_alts;
mod parse_mounts;
mod parse_snaps;
mod recursive;
mod snapshot_ops;
mod utility;

use crate::display::{display_exec, display_mounts_for_files};
use crate::interactive::interactive_exec;
use crate::lookup_versions::versions_lookup_exec;
use crate::parse_alts::precompute_alt_replicated;
use crate::parse_mounts::{get_common_snap_dir, parse_mounts_exec};
use crate::recursive::display_recursive_wrapper;
use crate::snapshot_ops::take_snapshot;
use crate::utility::{httm_is_dir, install_hot_keys, read_stdin, HttmError, PathData};

pub const ZFS_FSTYPE: &str = "zfs";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";
pub const ROOT_DIRECTORY: &str = "/";
pub const PROC_DIRECTORY: &str = "/proc";
pub const SYS_DIRECTORY: &str = "/sys";
pub const DEV_DIRECTORY: &str = "/dev";

pub const PHANTOM_DATE: SystemTime = SystemTime::UNIX_EPOCH;
pub const PHANTOM_SIZE: u64 = 0u64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilesystemType {
    Zfs,
    Btrfs,
}

#[derive(Debug, Clone, PartialEq)]
enum ExecMode {
    Interactive,
    DisplayRecursive,
    Display,
    SnapFileMount,
    LastSnap(RequestRelative),
    MountsForFiles,
}

#[derive(Debug, Clone, PartialEq)]
enum InteractiveMode {
    None,
    Browse,
    Select,
    Restore,
}

#[derive(Debug, Clone, PartialEq)]
enum DeletedMode {
    Disabled,
    DepthOfOne,
    Enabled,
    Only,
}

#[derive(Debug, Clone, PartialEq)]
enum DatasetCollection {
    AutoDetect(AutoDetectDatasets),
    UserDefined(UserDefinedDirs),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoDetectDatasets {
    // key: mount, val: (dataset/subvol, fstype)
    map_of_datasets: HashMap<PathBuf, (String, FilesystemType)>,
    // key: mount, val: snap locations on disk (e.g. /.zfs/snapshot/snap_8a86e4fc_prepApt/home)
    map_of_snaps: HashMap<PathBuf, Vec<PathBuf>>,
    // key: mount, val: alt dataset
    opt_map_of_alts: Option<HashMap<Arc<PathBuf>, DatasetsForSearch>>,
    vec_of_filter_dirs: Vec<PathBuf>,
    opt_common_snap_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserDefinedDirs {
    snap_dir: PathBuf,
    local_dir: PathBuf,
    fs_type: FilesystemType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotDatasetType {
    MostProximate,
    AltReplicated,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestRelative {
    Absolute,
    Relative,
}

fn parse_args() -> ArgMatches {
    clap::Command::new(crate_name!())
        .about("httm prints the size, date and corresponding locations of available unique versions of files residing on snapshots.  \
        May also be used interactively to select and restore from such versions, and even to snapshot datasets which contain certain files.")
        .version(crate_version!())
        .arg(
            Arg::new("INPUT_FILES")
                .help("in any non-interactive mode, put requested files here.  If you enter no files, \
                then httm will pause waiting for input on stdin(3). In any interactive mode, \
                this is the directory search path. If no directory is entered, \
                httm will use the current working directory.")
                .takes_value(true)
                .multiple_values(true)
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(1)
        )
        .arg(
            Arg::new("INTERACTIVE")
                .short('i')
                .long("interactive")
                .help("interactive browse and search a specified directory to display unique file versions.")
                .display_order(2)
        )
        .arg(
            Arg::new("SELECT")
                .short('s')
                .long("select")
                .help("interactive browse and search a specified directory to display unique file versions.  Continue to another dialog to select a snapshot version to dump to stdout(3).")
                .conflicts_with("RESTORE")
                .display_order(3)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .takes_value(true)
                .default_missing_value("copy")
                .possible_values(&["copy", "overwrite", "yolo"])
                .min_values(0)
                .require_equals(true)
                .help("interactive browse and search a specified directory to display unique file versions.  Continue to another dialog to select a snapshot version to restore.  \
                Default is a non-destructive \"copy\" to the current working directory with a new name, so as not to overwrite any \"live\" file version.  However, user may specify \"overwrite\" to restore to the same file location.")
                .conflicts_with("SELECT")
                .display_order(4)
        )
        .arg(
            Arg::new("DELETED_MODE")
                .short('d')
                .long("deleted")
                .takes_value(true)
                .default_missing_value("all")
                .possible_values(&["all", "single", "only"])
                .min_values(0)
                .require_equals(true)
                .help("show deleted files in interactive modes.  In non-interactive modes, do a search for all files deleted from a specified directory. \
                If \"--deleted only\" is specified, then, in interactive modes, non-deleted files will be excluded from the search. \
                If \"--deleted single\" is specified, then, deleted files behind deleted directories, \
                (files with a depth greater than one) will be ignored.")
                .display_order(5)
        )
        .arg(
            Arg::new("ALT_REPLICATED")
                .short('a')
                .long("alt-replicated")
                .help("automatically discover locally replicated datasets and list their snapshots as well.  \
                NOTE: Be certain such replicated datasets are mounted before use.  \
                httm will silently ignore unmounted datasets in the interactive modes.")
                .conflicts_with_all(&["SNAP_POINT", "LOCAL_DIR"])
                .display_order(6)
        )
        .arg(
            Arg::new("RECURSIVE")
                .short('R')
                .long("recursive")
                .conflicts_with_all(&["SNAP_FILE_MOUNT"])
                .help("recurse into the selected directory to find more files. Only available in interactive and deleted file modes.")
                .display_order(7)
        )
        .arg(
            Arg::new("EXACT")
                .short('e')
                .long("exact")
                .help("use exact pattern matching for searches in the interactive modes (in contrast to the default fuzzy-finder searching).")
                .display_order(8)
        )
        .arg(
            Arg::new("SNAP_FILE_MOUNT")
                .short('S')
                .long("snap")
                .visible_aliases(&["snap-file", "snapshot", "snap-file-mount"])
                .help("snapshot the mount point/s of the dataset/s which contains the input file/s. Note: This is a ZFS only option.")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE", "ALT_REPLICATED", "SNAP_POINT", "LOCAL_DIR"])
                .display_order(9)
        )
        .arg(
            Arg::new("MOUNT_FOR_FILE")
                .short('m')
                .long("mount-for-file")
                .visible_alias("mount")
                .help("display the mount point/s of the dataset/s which contains the input file/s.")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE"])
                .display_order(10)
        )
        .arg(
            Arg::new("LAST_SNAP")
                .short('l')
                .long("last-snap")
                .takes_value(true)
                .default_missing_value("abs")
                .possible_values(&["abs", "absolute", "rel", "relative"])
                .min_values(0)
                .require_equals(true)
                .help("automatically select and print the path of last-in-time unique snapshot version for the input file.  \
                May also be used as a shortcut to restore from such last version when used with the \"--restore\", or \"-r\", flag.  \
                Default is to return the absolute last-in-time but user may also request the last unique file version relative to the \"live\" version by appending \"relative\" to the flag.")
                .conflicts_with_all(&["SNAP_FILE_MOUNT", "MOUNT_FOR_FILE", "ALT_REPLICATED", "SNAP_POINT", "LOCAL_DIR", "NOT_SO_PRETTY"])
                .display_order(11)
        )
        .arg(
            Arg::new("NO_FILTER")
                .long("no-filter")
                .help("by default, in the interactive modes, httm will filter out results from non-supported datasets (like ext4, tmpfs, procfs, sysfs, or devtmpfs), and in common snapshot paths.  \
                Here, one may select to disable such filtering.  httm, however, should always show the input path, and results from behind any input path when that path is searched.")
                .display_order(12)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .visible_alias("newline")
                .help("display the snapshot locations only, without extraneous information, delimited by a NEWLINE character.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"])
                .display_order(13)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("display the snapshot locations only, without extraneous information, delimited by a NULL character.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"])
                .display_order(14)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .visible_aliases(&["tabs", "plain-jane"])
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(15)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .visible_aliases(&["dead", "disco"])
                .help("only display information concerning snapshot versions (display no information regarding 'live' versions of files or directories).")
                .display_order(16)
        )
        .arg(
            Arg::new("NO_SNAP")
                .long("no-snap")
                .visible_aliases(&["undead"])
                .help("only display information concerning 'pseudo-live' versions in Display Recursive mode (in --deleted, --recursive, but non-interactive modes).  \
                Useful for finding only the \"files that once were\" and displaying only those pseudo-live/undead files.")
                .requires("RECURSIVE")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE", "SNAP_FILE_MOUNT", "LAST_SNAP", "NOT_SO_PRETTY"])
                .display_order(17)
        )
        .arg(
            Arg::new("REMOTE_DIR")
                .long("remote-dir")
                .visible_aliases(&["remote", "snap-point"])
                .help("ordinarily httm will automatically choose your dataset root directory (the most proximate ancestor directory which contains a snapshot directory), \
                but here you may manually specify that mount point for ZFS (directory which contains a \".zfs\" directory) or btrfs-snapper (directory which contains a \".snapshots\" directory), \
                such as the local mount point for a remote share.  You may also set via the HTTM_REMOTE_DIR environment variable.  \
                Note: Use of both \"remote\" and \"local\" are not always necessary to view versions on remote shares.  \
                These options *are necessary* if you want to view snapshot versions from within the local directory you back up to your remote share, \
                however, httm can also automatically detect ZFS and btrfs-snapper datasets mounted as AFP, SMB, and NFS remote shares, if you browse that remote share where it is locally mounted.")
                .conflicts_with("ALT_REPLICATED")
                .takes_value(true)
                .display_order(18)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .visible_alias("local")
                .help("used with \"remote\" to determine where the corresponding live root filesystem of the dataset is.  \
                Put more simply, the \"local\" is the directory you backup to your \"remote\".  If not set, httm defaults to your current working directory.  \
                You may also set via the environment variable HTTM_LOCAL_DIR.")
                .conflicts_with("ALT_REPLICATED")
                .requires("SNAP_POINT")
                .takes_value(true)
                .display_order(19)
        )
        .arg(
            Arg::new("DEBUG")
                .long("debug")
                .help("print configuration and debugging info")
                .display_order(20)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(21)
        )
        .get_matches()
}

#[derive(Debug, Clone)]
pub struct Config {
    paths: Vec<PathData>,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live: bool,
    opt_recursive: bool,
    opt_exact: bool,
    opt_overwrite: bool,
    opt_no_filter: bool,
    opt_no_snap: bool,
    opt_debug: bool,
    datasets_of_interest: Vec<SnapshotDatasetType>,
    exec_mode: ExecMode,
    dataset_collection: DatasetCollection,
    deleted_mode: DeletedMode,
    interactive_mode: InteractiveMode,
    pwd: PathData,
    requested_dir: Option<PathData>,
}

impl Config {
    fn from(
        matches: ArgMatches,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync + 'static>> {
        if matches.is_present("ZSH_HOT_KEYS") {
            install_hot_keys()?
        }

        let opt_zeros = matches.is_present("ZEROS");
        let mut opt_raw = matches.is_present("RAW");
        let opt_no_pretty = matches.is_present("NOT_SO_PRETTY");
        let opt_recursive = matches.is_present("RECURSIVE");
        let opt_exact = matches.is_present("EXACT");
        let opt_no_live = matches.is_present("NO_LIVE");
        let opt_no_filter = matches.is_present("NO_FILTER");
        let opt_no_snap = matches.is_present("NO_SNAP");
        let opt_debug = matches.is_present("DEBUG");
        let opt_overwrite = matches!(
            matches.value_of("RESTORE"),
            Some("overwrite") | Some("yolo")
        );

        // force a raw mode if one is not set for no_snap mode
        if opt_no_snap && !opt_raw && !opt_zeros {
            opt_raw = true
        }

        let mut deleted_mode = match matches.value_of("DELETED_MODE") {
            Some("") | Some("all") => DeletedMode::Enabled,
            Some("single") => DeletedMode::DepthOfOne,
            Some("only") => DeletedMode::Only,
            _ => DeletedMode::Disabled,
        };

        let mut exec_mode = if matches.is_present("LAST_SNAP") {
            let request_relative = if matches!(
                matches.value_of("LAST_SNAP"),
                Some("rel") | Some("relative")
            ) {
                RequestRelative::Relative
            } else {
                RequestRelative::Absolute
            };
            ExecMode::LastSnap(request_relative)
        } else if matches.is_present("MOUNT_FOR_FILE") {
            ExecMode::MountsForFiles
        } else if matches.is_present("SNAP_FILE_MOUNT") {
            ExecMode::SnapFileMount
        } else if matches.is_present("INTERACTIVE")
            || matches.is_present("SELECT")
            || matches.is_present("RESTORE")
        {
            ExecMode::Interactive
        } else if deleted_mode != DeletedMode::Disabled {
            ExecMode::DisplayRecursive
        } else {
            // no need for deleted file modes in a non-interactive/display recursive setting
            deleted_mode = DeletedMode::Disabled;
            ExecMode::Display
        };

        let env_snap_dir = if std::env::var_os("HTTM_REMOTE_DIR").is_some() {
            std::env::var_os("HTTM_REMOTE_DIR")
        } else {
            // legacy env var name
            std::env::var_os("HTTM_SNAP_POINT")
        };
        let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");

        let raw_snap_dir = if let Some(value) = matches.value_of_os("REMOTE_DIR") {
            Some(value.to_os_string())
        } else {
            env_snap_dir
        };

        let raw_local_dir = if let Some(value) = matches.value_of_os("LOCAL_DIR") {
            Some(value.to_os_string())
        } else {
            env_local_dir
        };

        let interactive_mode = if matches.is_present("RESTORE") {
            InteractiveMode::Restore
        } else if matches.is_present("SELECT") {
            InteractiveMode::Select
        } else if matches.is_present("INTERACTIVE") {
            InteractiveMode::Browse
        } else {
            InteractiveMode::None
        };

        if opt_recursive {
            if matches!(exec_mode, ExecMode::Display) {
                return Err(
                    HttmError::new("Recursive search not available in Display Mode.").into(),
                );
            }
        } else if opt_no_filter {
            return Err(HttmError::new(
                "No filter mode only available when recursive search is enabled.",
            )
            .into());
        }

        // current working directory will be helpful in a number of places
        let pwd = if let Ok(pwd) = std::env::current_dir() {
            if let Ok(path) = PathBuf::from(&pwd).canonicalize() {
                PathData::from(path.as_path())
            } else {
                return Err(HttmError::new(
                    "Could not obtain a canonical path for your working directory",
                )
                .into());
            }
        } else {
            return Err(HttmError::new(
                "Working directory does not exist or your do not have permissions to access it.",
            )
            .into());
        };

        // paths are immediately converted to our PathData struct
        let mut paths: Vec<PathData> = if let Some(input_files) =
            matches.values_of_os("INPUT_FILES")
        {
            input_files
                .par_bridge()
                .map(Path::new)
                // canonicalize() on a deleted relative path will not exist,
                // so we have to join with the pwd to make a path that
                // will exist on a snapshot
                .map(|path| canonicalize(path).unwrap_or_else(|_| pwd.clone().path_buf.join(path)))
                .map(|path| PathData::from(path.as_path()))
                .collect()
        } else {
            match exec_mode {
                // setting pwd as the path, here, keeps us from waiting on stdin when in certain modes
                //  is more like Interactive and DisplayRecursive in this respect in requiring only one
                // input, and waiting on one input from stdin is pretty silly
                ExecMode::Interactive | ExecMode::DisplayRecursive | ExecMode::LastSnap(_) => {
                    vec![pwd.clone()]
                }
                ExecMode::Display | ExecMode::SnapFileMount | ExecMode::MountsForFiles => {
                    read_stdin()?
                        .par_iter()
                        .map(|string| PathData::from(Path::new(&string)))
                        .collect()
                }
            }
        };

        // deduplicate pathdata and sort if in display mode --
        // so input of ./.z* and ./.zshrc will only print ./.zshrc once
        paths = if paths.len() > 1 {
            paths.par_sort_by_key(|pathdata| pathdata.path_buf.clone());
            // dedup needs to be sorted/ordered first to work (not like a HashMap)
            paths.dedup_by_key(|pathdata| pathdata.path_buf.clone());

            paths
        } else {
            paths
        };

        // for exec_modes in which we can only take a single directory, process how we handle those here
        let requested_dir: Option<PathData> = match exec_mode {
            ExecMode::Interactive | ExecMode::DisplayRecursive | ExecMode::LastSnap(_) => {
                match paths.len() {
                    0 => Some(pwd.clone()),
                    1 => {
                        // safe to index as we know the paths len is 1
                        let pathdata = &paths[0];

                        // use our bespoke is_dir fn for determining whether a dir here see pub httm_is_dir
                        if httm_is_dir(pathdata) {
                            Some(pathdata.clone())
                        // and then we take all comers here because may be a deleted file that DNE on a live version
                        } else {
                            match exec_mode {
                                ExecMode::Interactive | ExecMode::LastSnap(_) => {
                                    match interactive_mode {
                                        InteractiveMode::Browse | InteractiveMode::None => {
                                            // doesn't make sense to have a non-dir in these modes
                                            return Err(HttmError::new(
                                                        "Path specified is not a directory, and therefore not suitable for browsing.",
                                                    )
                                                    .into());
                                        }
                                        InteractiveMode::Restore | InteractiveMode::Select => {
                                            // non-dir file will just cause us to skip the lookup phase
                                            None
                                        }
                                    }
                                }
                                // silently disable DisplayRecursive when path given is not a directory
                                // switch to a standard Display mode
                                ExecMode::DisplayRecursive => {
                                    exec_mode = ExecMode::Display;
                                    deleted_mode = DeletedMode::Disabled;
                                    None
                                }
                                _ => unreachable!(),
                            }
                        }
                    }
                    n if n > 1 => return Err(HttmError::new(
                        "May only specify one path in the display recursive or interactive modes.",
                    )
                    .into()),
                    _ => {
                        unreachable!()
                    }
                }
            }
            ExecMode::Display | ExecMode::SnapFileMount | ExecMode::MountsForFiles => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                None
            }
        };

        // here we determine how we will obtain our snap point -- has the user defined it
        // or will we find it by searching the native filesystem? if searching a native filesystem,
        // we will obtain a map of datasets, a map of snapshot directory, and possibly a map of
        // alternate filesystems if the user request early to avoid looking up later.
        let (datasets_of_interest, dataset_collection) = if let Some(raw_value) = raw_snap_dir {
            // user defined dir exists?: check that path contains the hidden snapshot directory
            let snap_dir = PathBuf::from(raw_value);

            // little sanity check -- make sure the user defined snap dir exist
            if snap_dir.metadata().is_err() {
                return Err(HttmError::new(
                    "Manually set snap point directory does not exist.  Perhaps it is not already mounted?",
                )
                .into());
            }

            // set fstype, known by whether there is a ZFS hidden snapshot dir in the root dir
            let fs_type = if snap_dir.join(ZFS_SNAPSHOT_DIRECTORY).metadata().is_ok() {
                FilesystemType::Zfs
            } else if snap_dir
                .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
                .metadata()
                .is_ok()
            {
                FilesystemType::Btrfs
            } else {
                return Err(HttmError::new(
                        "User defined snap point is only available for ZFS datasets and btrfs datasets snapshot-ed via snapper.",
                    )
                    .into());
            };

            // local relative dir can be set at cmdline or as an env var, but defaults to current working directory
            let local_dir = if let Some(value) = raw_local_dir {
                let local_dir: PathBuf = PathBuf::from(value);

                // little sanity check -- make sure the user defined local dir exist
                if local_dir.metadata().is_ok() {
                    local_dir
                } else {
                    return Err(HttmError::new(
                        "Manually set local relative directory does not exist.  Please try another.",
                    )
                    .into());
                }
            } else {
                pwd.path_buf.clone()
            };

            (
                // always ignore opt_alt_replicated in UserDefinedDirs mode
                vec![SnapshotDatasetType::MostProximate],
                DatasetCollection::UserDefined(UserDefinedDirs {
                    snap_dir,
                    local_dir,
                    fs_type,
                }),
            )
        } else {
            let (map_of_datasets, map_of_snaps, vec_of_filter_dirs) = parse_mounts_exec()?;

            // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
            let opt_common_snap_dir = get_common_snap_dir(&map_of_datasets, &map_of_snaps);

            // only create a map of alts if necessary
            let opt_map_of_alts = if matches.is_present("ALT_REPLICATED") {
                Some(precompute_alt_replicated(&map_of_datasets))
            } else {
                None
            };

            let datasets_of_interest = if matches.is_present("ALT_REPLICATED") {
                vec![
                    SnapshotDatasetType::AltReplicated,
                    SnapshotDatasetType::MostProximate,
                ]
            } else {
                vec![SnapshotDatasetType::MostProximate]
            };

            (
                datasets_of_interest,
                DatasetCollection::AutoDetect(AutoDetectDatasets {
                    map_of_datasets,
                    map_of_snaps,
                    opt_map_of_alts,
                    vec_of_filter_dirs,
                    opt_common_snap_dir,
                }),
            )
        };

        let config = Config {
            paths,
            opt_raw,
            opt_zeros,
            opt_no_pretty,
            opt_no_live,
            opt_recursive,
            opt_exact,
            opt_overwrite,
            opt_no_filter,
            opt_no_snap,
            opt_debug,
            datasets_of_interest,
            dataset_collection,
            exec_mode,
            deleted_mode,
            interactive_mode,
            pwd,
            requested_dir,
        };

        Ok(config)
    }
}

fn main() {
    match exec() {
        Ok(_) => std::process::exit(0),
        Err(error) => {
            eprintln!("Error: {}", error);
            std::process::exit(1)
        }
    }
}

fn exec() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get our program args and generate a config for use
    // everywhere else
    let arg_matches = parse_args();
    let config = Config::from(arg_matches)?;

    if config.opt_debug {
        eprintln!("{:#x?}", config);
    }

    // this handles the basic ExecMode::Display case, other process elsewhere
    let snaps_and_live_set = match config.exec_mode {
        // ExecMode::Interactive may return back to this function to be printed
        // from an interactive browse must get the paths to print to display, or continue
        // to select or restore functions
        //
        // ExecMode::LastSnap will never return back, its a shortcut to select and restore themselves
        ExecMode::Interactive | ExecMode::LastSnap(_) => {
            let browse_result = &interactive_exec(&config)?;
            versions_lookup_exec(&config, browse_result)?
        }
        // ExecMode::Display will be just printed, we already know the paths
        ExecMode::Display => versions_lookup_exec(&config, &config.paths)?,
        // ExecMode::DisplayRecursive and ExecMode::SnapFileMount won't ever return back to this function
        ExecMode::DisplayRecursive => display_recursive_wrapper(&config)?,
        ExecMode::SnapFileMount => take_snapshot(&config)?,
        // ExecMode::MountsForFiles will print it output elsewhere as it's different from normal display output
        ExecMode::MountsForFiles => {
            display_mounts_for_files(&config)?;
            std::process::exit(0)
        }
    };

    // and display
    let output_buf = display_exec(&config, &snaps_and_live_set)?;
    print_output_buf(output_buf)?;

    Ok(())
}
