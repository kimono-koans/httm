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

mod config_helper;
mod deleted;
mod display;
mod interactive;
mod library;
mod lookup;

use crate::config_helper::{install_hot_keys, list_all_filesystems};
use crate::deleted::display_recursive_exec;
use crate::display::display_exec;
use crate::interactive::interactive_exec;
use crate::library::{httm_is_dir, read_stdin};
use crate::lookup::lookup_exec;

use clap::{Arg, ArgMatches};
use fxhash::FxHashSet as HashSet;
use rayon::prelude::*;
use std::fs::canonicalize;
use std::{
    error::Error,
    fmt,
    fs::{DirEntry, Metadata},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug)]
pub struct HttmError {
    details: String,
}

impl HttmError {
    fn new(msg: &str) -> HttmError {
        HttmError {
            details: msg.to_owned(),
        }
    }
    fn with_context(msg: &str, err: Box<dyn Error + 'static>) -> HttmError {
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    system_time: SystemTime,
    size: u64,
    path_buf: PathBuf,
    is_phantom: bool,
}

impl From<&Path> for PathData {
    fn from(path: &Path) -> PathData {
        let metadata_res = std::fs::symlink_metadata(path);
        PathData::from_parts(path, metadata_res)
    }
}

impl From<&DirEntry> for PathData {
    fn from(dir_entry: &DirEntry) -> PathData {
        let metadata_res = dir_entry.metadata();
        let path = dir_entry.path();
        PathData::from_parts(&path, metadata_res)
    }
}

impl PathData {
    fn from_parts(path: &Path, metadata_res: Result<Metadata, std::io::Error>) -> PathData {
        let absolute_path: PathBuf = if path.is_relative() {
            if let Ok(canonical_path) = path.canonicalize() {
                canonical_path
            } else {
                // canonicalize() on any path that DNE will throw an error
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
    fn is_dir(&self) -> bool {
        httm_is_dir(&self.path_buf)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ExecMode {
    Interactive,
    DisplayRecursive,
    Display,
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
    Enabled,
    Only,
}

#[derive(Debug, Clone)]
enum SnapPoint {
    Native(Vec<FilesystemAndMount>),
    UserDefined(UserDefinedDirs),
}

#[derive(Debug, Clone)]
pub struct FilesystemAndMount {
    filesystem: String,
    mount: String,
}

#[derive(Debug, Clone)]
pub struct UserDefinedDirs {
    snap_dir: PathBuf,
    local_dir: PathBuf,
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
    exec_mode: ExecMode,
    snap_point: SnapPoint,
    deleted_mode: DeletedMode,
    interactive_mode: InteractiveMode,
    pwd: PathData,
    requested_dir: PathData,
}

impl Config {
    fn from(
        matches: ArgMatches,
    ) -> Result<Config, Box<dyn std::error::Error + Send + Sync + 'static>> {
        if matches.is_present("ZSH_HOT_KEYS") {
            install_hot_keys()?
        }
        let opt_zeros = matches.is_present("ZEROS");
        let opt_raw = matches.is_present("RAW");
        let opt_no_pretty = matches.is_present("NOT_SO_PRETTY");
        let opt_no_live_vers = matches.is_present("NO_LIVE");
        let opt_recursive = matches.is_present("RECURSIVE");
        let mut deleted_mode = match matches.value_of("DELETED") {
            None => DeletedMode::Disabled,
            Some("") => DeletedMode::Enabled,
            Some("only") | Some("ONLY") => DeletedMode::Only,
            // invalid value to not specify one of the above
            _ => unreachable!(),
        };
        let mut exec_mode = if matches.is_present("INTERACTIVE")
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
        let env_snap_dir = std::env::var_os("HTTM_SNAP_POINT");
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

        if opt_recursive && exec_mode == ExecMode::Display {
            return Err(
                HttmError::new("Recursive search feature only allowed in select modes.").into(),
            );
        }

        // current working directory will be helpful in a number of places
        let pwd = if let Ok(pwd) = std::env::var("PWD") {
            if let Ok(path) = PathBuf::from(&pwd).canonicalize() {
                PathData::from(path.as_path())
            } else {
                return Err(HttmError::new(
                    "Working directory, as set in your environment, does not appear to exist",
                )
                .into());
            }
        } else {
            return Err(HttmError::new("Working directory is not set in your environment.").into());
        };

        // where is the hidden snapshot directory located?
        // just below we ask whether the user has defined that place
        let raw_snap_var = if let Some(value) = matches.value_of_os("SNAP_POINT") {
            Some(value.to_os_string())
        } else {
            env_snap_dir
        };

        // here we determine how we will obtain our snap point -- has the user defined it
        // or will we find it by searching the native filesystem?
        let (opt_alt_replicated, snap_point) = if let Some(raw_value) = raw_snap_var {
            // user defined dir exists?: check that path contains the hidden snapshot directory
            let path = PathBuf::from(raw_value);
            let hidden_snap_dir = path.join(".zfs").join("snapshot");

            // little sanity check -- make sure the user defined snap dir exist
            let snap_dir = if hidden_snap_dir.metadata().is_ok() {
                path
            } else {
                return Err(HttmError::new(
                    "Manually set mountpoint does not contain a hidden ZFS directory.  Please mount a ZFS directory there or try another mountpoint.",
                ).into());
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
                }),
            )
        } else {
            let mount_collection: Vec<FilesystemAndMount> = list_all_filesystems()?;
            (
                matches.is_present("ALT_REPLICATED"),
                SnapPoint::Native(mount_collection),
            )
        };

        // paths are immediately converted to our PathData struct
        let mut paths: Vec<PathData> = if let Some(input_files) =
            matches.values_of_os("INPUT_FILES")
        {
            input_files
                .into_iter()
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
        } else if exec_mode == ExecMode::Display {
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
        paths = if exec_mode == ExecMode::Display && paths.len() > 1 {
            let mut unique_paths: HashSet<PathData> = HashSet::default();

            paths.into_iter().for_each(|pathdata| {
                let _ = unique_paths.insert(pathdata);
            });

            let mut sorted: Vec<PathData> = unique_paths.into_iter().collect();
            sorted.par_sort_unstable_by_key(|pathdata| (pathdata.system_time, pathdata.size));

            sorted
        } else {
            paths
        };

        // for exec_modes in which we can only take a single directory, process how we handle those here
        let requested_dir: PathData = match exec_mode {
            ExecMode::Interactive => {
                match paths.len() {
                    0 => pwd.clone(),
                    1 => {
                        // impossible to panic, because we are indexing to 0 on a len we know to be 1
                        let pathdata = paths.get(0).unwrap();
                        // use our bespoke is_dir fn for determining whether a dir here see pub httm_is_dir
                        if pathdata.is_dir() {
                            pathdata.to_owned()
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
                                    pathdata.to_owned()
                                }
                            }
                        }
                    }
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
            ExecMode::DisplayRecursive => {
                // paths should never be empty for ExecMode::DisplayRecursive
                //
                // we only want one dir for a ExecMode::DisplayRecursive run, else
                // we should run in ExecMode::Display mode
                match paths.len() {
                    1 => match paths.get(0) {
                        Some(pathdata) if pathdata.is_dir() => pathdata.to_owned(),
                        _ => {
                            exec_mode = ExecMode::Display;
                            deleted_mode = DeletedMode::Disabled;
                            pwd.clone()
                        }
                    },
                    _ => {
                        // paths should never be empty, but here we make sure
                        exec_mode = ExecMode::Display;
                        deleted_mode = DeletedMode::Disabled;
                        pwd.clone()
                    }
                }
            }
            ExecMode::Display => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                pwd.clone()
            }
        };

        let config = Config {
            paths,
            opt_alt_replicated,
            opt_raw,
            opt_zeros,
            opt_no_pretty,
            opt_no_live_vers,
            opt_recursive,
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
    clap::Command::new("httm")
        .about("\nBy default, httm will display non-interactive information about unique file versions contained on ZFS snapshots.\n\n\
        You may also select from the various interactive modes below to browse for, select, and/or restore files.")
        .version("0.9.8") 
        .arg(
            Arg::new("INPUT_FILES")
                .help("in the default, non-interactive mode, put requested files here.  If you enter no files, \
                then httm will pause waiting for input on stdin(3).  In any interactive mode, this is the directory search path. \
                If no directory is entered, httm will use the current working directory.")
                .takes_value(true)
                .multiple_values(true)
                .display_order(1)
        )
        .arg(
            Arg::new("INTERACTIVE")
                .short('i')
                .long("interactive")
                .help("interactively browse and search files.")
                .display_order(2)
        )
        .arg(
            Arg::new("SELECT")
                .short('s')
                .long("select")
                .help("interactively browse and search files.  Continue to another dialog to select a snapshot version.")
                .conflicts_with("RESTORE")
                .display_order(3)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .help("interactively browse and search files.  Continue to another dialog to select a snapshot version to restore.")
                .conflicts_with("SELECT")
                .display_order(4)
        )
        .arg(
            Arg::new("DELETED")
                .short('d')
                .long("deleted")
                .takes_value(true)
                .default_missing_value("")
                .possible_values(&["only", "ONLY", ""])
                .hide_possible_values(true)
                .help("show deleted files in interactive modes, or do a search for all such files, if a directory is specified. \
                If --deleted=only is specified, then, in interactive modes, non-deleted files will be excluded from the search.")
                .display_order(5)
        )
        .arg(
            Arg::new("ALT_REPLICATED")
                .short('a')
                .long("alt-replicated")
                .help("automatically discover an alternative locally replicated dataset and list its snapshots as well.  \
                NOTE: Make certain any replicated dataset is mounted before use, as httm will silently ignore any unmounted \
                datasets in the interactive modes.")
                .conflicts_with_all(&["SNAP_POINT", "LOCAL_DIR"])
                .display_order(6)
        )
        .arg(
            Arg::new("RECURSIVE")
                .short('R')
                .long("recursive")
                .help("recurse into selected directory to find more files. Only available in interactive and deleted file modes.")
                .display_order(7)
        )
        .arg(
            Arg::new("SNAP_POINT")
                .long("snap-point")
                .help("ordinarily httm will automatically choose your most immediate snapshot directory, \
                but here you may manually specify your own mount point for that directory, such as the mount point for a remote share.  \
                You can also set via the environment variable HTTM_SNAP_POINT.")
                .takes_value(true)
                .display_order(8)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .help("used with SNAP_POINT to determine where the corresponding live root of the ZFS snapshot dataset is.  If not set, \
                httm defaults to your current working directory.  You can also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("SNAP_POINT")
                .takes_value(true)
                .display_order(9)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .help("display the backup locations only, without extraneous information, delimited by a NEWLINE.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"])
                .display_order(10)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("display the backup locations only, without extraneous information, delimited by a NULL CHARACTER.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"])
                .display_order(11)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(12)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .help("only display information concerning snapshot versions, and no 'live' versions of files or directories.")
                .display_order(13)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(14)
        )
        .get_matches()
}

fn main() {
    if let Err(error) = exec() {
        eprintln!("Error: {}", error);
        std::process::exit(1)
    } else {
        std::process::exit(0)
    }
}

fn exec() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut out = std::io::stdout();

    // get our program args and generate a config for use
    // everywhere else
    let arg_matches = parse_args();
    let config = Config::from(arg_matches)?;

    let snaps_and_live_set = match config.exec_mode {
        // 1. Do our interactive lookup thing, or not, to obtain raw string paths
        // 2. Determine/lookup whether file matches any files on snapshots
        ExecMode::Interactive => lookup_exec(&config, &interactive_exec(&mut out, &config)?)?,
        ExecMode::Display => lookup_exec(&config, &config.paths)?,
        // display_recursive_exec is special as there is no need to run a lookup on files already on snapshots
        ExecMode::DisplayRecursive => display_recursive_exec(&config, &mut out)?,
    };

    // and display
    let output_buf = display_exec(&config, snaps_and_live_set)?;

    write!(out, "{}", output_buf)?;
    out.flush()?;

    Ok(())
}
