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
    error::Error,
    ffi::OsString,
    fmt,
    fs::{canonicalize, symlink_metadata, DirEntry, FileType, Metadata},
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
    time::SystemTime,
};

// so we may use AHashMap with Parallel Iterators!
// impl of AHashMap as (K,V) instead of (K,V,S) so we
// may use with collect() function, see rayon::FromParallelIterator
use ahash::AHasher;
pub type AHashBuildHasher = BuildHasherDefault<AHasher>;
pub type AHashMapSpecial<K, V> = std::collections::HashMap<K, V, AHashBuildHasher>;
use AHashMapSpecial as HashMap;

use clap::{crate_name, crate_version, Arg, ArgMatches};
use rayon::prelude::*;

mod deleted_lookup;
mod display;
mod interactive;
mod parse_mounts;
mod process_dirs;
mod snapshot_ops;
mod utility;
mod versions_lookup;

use crate::interactive::interactive_exec;
use crate::parse_mounts::{get_common_snap_dir, get_filesystems_list, precompute_alt_replicated};
use crate::process_dirs::display_recursive_wrapper;
use crate::snapshot_ops::take_snapshot;
use crate::utility::{httm_is_dir, install_hot_keys, read_stdin};
use crate::versions_lookup::get_versions_set;
use crate::{display::display_exec, interactive::interactive_select};

pub const ZFS_FSTYPE: &str = "zfs";
pub const BTRFS_FSTYPE: &str = "btrfs";
pub const SMB_FSTYPE: &str = "smbfs";
pub const NFS_FSTYPE: &str = "nfs";
pub const AFP_FSTYPE: &str = "afpfs";

pub const ZFS_HIDDEN_DIRECTORY: &str = ".zfs";
pub const ZFS_SNAPSHOT_DIRECTORY: &str = ".zfs/snapshot";
pub const BTRFS_SNAPPER_HIDDEN_DIRECTORY: &str = ".snapshots";
pub const BTRFS_SNAPPER_SUFFIX: &str = "snapshot";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilesystemType {
    Zfs,
    Btrfs,
}

#[derive(Debug)]
pub struct HttmError {
    details: String,
}

impl HttmError {
    fn new(msg: &str) -> Self {
        HttmError {
            details: msg.to_owned(),
        }
    }
    fn with_context(msg: &str, err: Box<dyn Error + 'static>) -> Self {
        let msg_plus_context = format!("{} : {:?}", msg, err);
        HttmError {
            details: msg_plus_context,
        }
    }
}

impl fmt::Display for HttmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

impl Error for HttmError {
    fn description(&self) -> &str {
        &self.details
    }
}

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct BasicDirEntryInfo {
    file_name: OsString,
    path: PathBuf,
    file_type: Option<FileType>,
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            file_name: dir_entry.file_name(),
            path: dir_entry.path(),
            file_type: dir_entry.file_type().ok(),
        }
    }
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    system_time: SystemTime,
    size: u64,
    path_buf: PathBuf,
    is_phantom: bool,
}

impl From<&Path> for PathData {
    fn from(path: &Path) -> Self {
        let metadata_res = symlink_metadata(path);
        PathData::from_parts(path, metadata_res)
    }
}

impl From<&DirEntry> for PathData {
    fn from(dir_entry: &DirEntry) -> Self {
        let metadata_res = dir_entry.metadata();
        let path = dir_entry.path();
        PathData::from_parts(&path, metadata_res)
    }
}

impl PathData {
    fn from_parts(path: &Path, metadata_res: Result<Metadata, std::io::Error>) -> Self {
        let absolute_path: PathBuf = if path.is_relative() {
            if let Ok(canonical_path) = path.canonicalize() {
                canonical_path
            } else {
                // canonicalize() on any path that DNE will throw an error
                //
                // in general we handle those cases elsewhere, like the ingest
                // of input files in Config::from for deleted relative paths, etc.
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };

        // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
        let (len, time, phantom) = match metadata_res {
            Ok(md) => {
                let len = md.len();
                let time = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let phantom = false;
                (len, time, phantom)
            }
            // this seems like a perfect place for a None value, as the file has no metadata,
            // however we will want certain iters to print the *request*, say for deleted files,
            // so we set up a dummy Some value just so we can have the path names we entered
            //
            // if we get a spurious example of no metadata in snapshot directories, we just ignore later
            Err(_) => {
                let len = 0u64;
                let time = SystemTime::UNIX_EPOCH;
                let phantom = true;
                (len, time, phantom)
            }
        };

        PathData {
            system_time: time,
            size: len,
            path_buf: absolute_path,
            is_phantom: phantom,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ExecMode {
    Interactive,
    DisplayRecursive,
    Display,
    SnapFileMount,
    LastSnap,
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

#[derive(Debug, Clone)]
enum SnapPoint {
    Native(NativeDatasets),
    UserDefined(UserDefinedDirs),
}

#[derive(Debug, Clone)]
pub struct NativeDatasets {
    // key: mount, val: (dataset/subvol, fstype)
    map_of_datasets: HashMap<PathBuf, (String, FilesystemType)>,
    // key: mount, val: snap locations on disk (e.g. /.zfs/snapshot/snap_8a86e4fc_prepApt/home)
    map_of_snaps: HashMap<PathBuf, Vec<PathBuf>>,
    // key: mount, val: alt dataset
    opt_map_of_alts: Option<HashMap<PathBuf, Vec<PathBuf>>>,
    opt_common_snap_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct UserDefinedDirs {
    snap_dir: PathBuf,
    local_dir: PathBuf,
    fs_type: FilesystemType,
    opt_common_snap_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Config {
    paths: Vec<PathData>,
    opt_alt_replicated: bool,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live_vers: bool,
    opt_recursive: bool,
    opt_exact: bool,
    opt_mount_for_file: bool,
    exec_mode: ExecMode,
    snap_point: SnapPoint,
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
        let opt_raw = matches.is_present("RAW");
        let opt_no_pretty = matches.is_present("NOT_SO_PRETTY");
        let opt_recursive = matches.is_present("RECURSIVE");
        let opt_exact = matches.is_present("EXACT");
        let opt_mount_for_file = matches.is_present("MOUNT_FOR_FILE");
        let opt_no_live_vers = matches.is_present("NO_LIVE") || opt_mount_for_file;

        let mut deleted_mode = match matches.value_of("DELETED_MODE") {
            None => DeletedMode::Disabled,
            Some("") | Some("all") => DeletedMode::Enabled,
            Some("single") => DeletedMode::DepthOfOne,
            Some("only") => DeletedMode::Only,
            // invalid value to not specify one of the above
            _ => unreachable!(),
        };

        let mut exec_mode = if matches.is_present("LAST_SNAP") {
            ExecMode::LastSnap
        } else if matches.is_present("SNAP_FILE_MOUNT") {
            ExecMode::SnapFileMount
        } else if matches.is_present("INTERACTIVE")
            || matches.is_present("RESTORE")
            || matches.is_present("SELECT")
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

        let interactive_mode = if matches.is_present("RESTORE") {
            InteractiveMode::Restore
        } else if matches.is_present("SELECT") {
            InteractiveMode::Select
        } else if matches.is_present("INTERACTIVE") {
            InteractiveMode::Browse
        } else {
            InteractiveMode::None
        };

        if opt_recursive && (exec_mode == ExecMode::Display || exec_mode == ExecMode::SnapFileMount)
        {
            return Err(
                HttmError::new("Recursive search feature only allowed in select modes.").into(),
            );
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

        // where is the hidden snapshot directory located?
        // just below we ask whether the user has defined that place
        let raw_snap_var = if let Some(value) = matches.value_of_os("REMOTE_DIR") {
            Some(value.to_os_string())
        } else {
            env_snap_dir
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

        // setting pwd as the path, here, keeps us from waiting on stdin when in non-Display modes
        } else if exec_mode == ExecMode::Interactive || exec_mode == ExecMode::DisplayRecursive {
            vec![pwd.clone()]
        } else if exec_mode == ExecMode::Display || exec_mode == ExecMode::SnapFileMount {
            read_stdin()?
                .iter()
                .par_bridge()
                .map(|string| PathData::from(Path::new(&string)))
                .collect()
        } else {
            unreachable!()
        };

        // deduplicate pathdata and sort if in display mode --
        // so input of ./.z* and ./.zshrc will only print ./.zshrc once
        paths = if paths.len() > 1
            && (exec_mode == ExecMode::Display || exec_mode == ExecMode::SnapFileMount)
        {
            paths.par_sort_by_key(|pathdata| pathdata.path_buf.clone());
            paths.dedup_by_key(|pathdata| pathdata.path_buf.clone());

            paths
        } else {
            paths
        };

        // for exec_modes in which we can only take a single directory, process how we handle those here
        let requested_dir: Option<PathData> = match exec_mode {
            ExecMode::Interactive => {
                match paths.len() {
                    0 => Some(pwd.clone()),
                    1 => match paths.get(0) {
                        Some(pathdata) => {
                            // use our bespoke is_dir fn for determining whether a dir here see pub httm_is_dir
                            if httm_is_dir(pathdata) {
                                Some(pathdata.clone())
                            // and then we take all comers here because may be a deleted file that DNE on a live version
                            } else {
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
                        }
                        _ => unreachable!(),
                    },
                    n if n > 1 => {
                        return Err(HttmError::new(
                            "May only specify one path in interactive mode.",
                        )
                        .into())
                    }
                    _ => {
                        unreachable!()
                    }
                }
            }
            ExecMode::DisplayRecursive | ExecMode::LastSnap => {
                // paths should never be empty for ExecMode::DisplayRecursive
                //
                // we only want one dir for a ExecMode::DisplayRecursive run, else
                // we should run in ExecMode::Display mode
                match paths.len() {
                    0 => Some(pwd.clone()),
                    1 => match exec_mode {
                        ExecMode::LastSnap => paths.get(0).cloned(),
                        _ => match paths.get(0) {
                            Some(pathdata) if httm_is_dir(pathdata) => Some(pathdata.clone()),
                            _ => {
                                exec_mode = ExecMode::Display;
                                deleted_mode = DeletedMode::Disabled;
                                None
                            }
                        },
                    },
                    n if n > 1 => {
                        return Err(HttmError::new(
                            "May only specify one path in display recursive or last snap modes.",
                        )
                        .into())
                    }
                    _ => {
                        unreachable!()
                    }
                }
            }
            ExecMode::Display | ExecMode::SnapFileMount => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                None
            }
        };

        // here we determine how we will obtain our snap point -- has the user defined it
        // or will we find it by searching the native filesystem?
        let (opt_alt_replicated, snap_point) = if let Some(raw_value) = raw_snap_var {
            if matches.is_present("ALT_REPLICATED") {
                return Err(HttmError::new(
                    "Alternate replicated datasets are not available for search, when the user defines a snap point.",
                )
                .into());
            }

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
            let (fs_type, opt_common_snap_dir) = if snap_dir
                .join(ZFS_SNAPSHOT_DIRECTORY)
                .metadata()
                .is_ok()
            {
                (FilesystemType::Zfs, None)
            } else if snap_dir
                .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
                .metadata()
                .is_ok()
            {
                (FilesystemType::Btrfs, None)
            } else {
                return Err(HttmError::new(
                        "User defined snap point is only available for ZFS datasets and btrfs datasets snapshot-ed via snapper.",
                    )
                    .into());
            };

            // has the user has defined a corresponding local relative directory?
            let raw_local_var = if let Some(raw_value) = matches.value_of_os("LOCAL_DIR") {
                Some(raw_value.to_os_string())
            } else {
                env_local_dir
            };

            // local relative dir can be set at cmdline or as an env var, but defaults to current working directory
            let local_dir = if let Some(value) = raw_local_var {
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
                // always set opt_alt_replicated to false in UserDefinedDirs mode
                false,
                SnapPoint::UserDefined(UserDefinedDirs {
                    snap_dir,
                    local_dir,
                    fs_type,
                    opt_common_snap_dir,
                }),
            )
        } else {
            let (map_of_datasets, map_of_snaps) = get_filesystems_list()?;

            // for a collection of btrfs mounts, indicates a common snapshot directory to ignore
            let opt_common_snap_dir = get_common_snap_dir(&map_of_datasets, &map_of_snaps);

            // only create a map of alts if necessary
            let opt_map_of_alts = if matches.is_present("ALT_REPLICATED") {
                Some(precompute_alt_replicated(&map_of_datasets))
            } else {
                None
            };

            (
                matches.is_present("ALT_REPLICATED"),
                SnapPoint::Native(NativeDatasets {
                    map_of_datasets,
                    map_of_snaps,
                    opt_map_of_alts,
                    opt_common_snap_dir,
                }),
            )
        };

        let config = Config {
            paths,
            opt_alt_replicated,
            opt_raw,
            opt_zeros,
            opt_no_pretty,
            opt_no_live_vers,
            opt_recursive,
            opt_exact,
            opt_mount_for_file,
            snap_point,
            exec_mode,
            deleted_mode,
            interactive_mode,
            pwd,
            requested_dir,
        };

        Ok(config)
    }
}

fn parse_args() -> ArgMatches {
    clap::Command::new(crate_name!())
        .about("\nhttm prints the size, date and corresponding locations of available unique versions of files residing on snapshots.\n\n\
        httm can also be used interactively to select and restore from such versions, and even snapshot datasets which contain certain files.")
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
                .help("interactive browse and search a specified directory to display unique file versions.  Continue to another dialog to select a snapshot version to restore.")
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
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE", "NOT_SO_PRETTY"])
                .display_order(10)
        )
        .arg(
            Arg::new("LAST_SNAP")
                .short('l')
                .long("last-snap")
                .help("automatically select and print the path of last snapshot version for the input file.  \
                Can also be used to more quickly restore from such version with the \"--restore\", or \"-r\", flag.")
                .conflicts_with_all(&["INTERACTIVE"])
                .display_order(11)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .visible_alias("newline")
                .help("display the snapshot locations only, without extraneous information, delimited by a NEWLINE.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"])
                .display_order(12)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("display the snapshot locations only, without extraneous information, delimited by a NULL CHARACTER.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"])
                .display_order(13)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .visible_aliases(&["tabs", "plain-jane"])
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(14)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .visible_aliases(&["dead", "disco"])
                .help("only display information concerning snapshot versions (display no information regarding 'live' versions of files or directories).")
                .display_order(15)
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
                .takes_value(true)
                .display_order(16)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .visible_alias("local")
                .help("used with \"remote\" to determine where the corresponding live root filesystem of the dataset is.  \
                Put more simply, the \"local\" is the directory you backup to your \"remote\".  If not set, httm defaults to your current working directory.  \
                You may also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("SNAP_POINT")
                .takes_value(true)
                .display_order(17)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(18)
        )
        .get_matches()
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

    // this handles the basic ExecMode::Display case, other process elsewhere
    let snaps_and_live_set = match config.exec_mode {
        // ExecMode::Interactive might, and Display will, return back to this function to be printed
        // 1. Do our interactive lookup thing, or not, to obtain raw string paths
        // 2. Determine/lookup whether file matches any files on snapshots
        ExecMode::Interactive => get_versions_set(&config, &interactive_exec(&config)?)?,
        ExecMode::Display => get_versions_set(&config, &config.paths)?,
        // ExecMode::DisplayRecursive and ExecMode::SnapFileMount won't ever return back to this function
        ExecMode::DisplayRecursive => display_recursive_wrapper(&config)?,
        ExecMode::SnapFileMount => take_snapshot(&config)?,
        ExecMode::LastSnap => {
            interactive_select(&config, &config.paths)?;
            unreachable!();
        }
    };

    // and display
    let output_buf = display_exec(&config, snaps_and_live_set)?;
    print!("{}", output_buf);

    Ok(())
}
