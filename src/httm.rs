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

mod deleted;
mod display;
mod interactive;
mod lookup;

use crate::deleted::deleted_exec;
use crate::display::display_exec;
use crate::interactive::interactive_exec;
use crate::lookup::lookup_exec;

use clap::{Arg, ArgMatches};
use fxhash::FxHashMap as HashMap;
use rayon::prelude::*;
use std::{
    env,
    error::Error,
    fmt,
    fs::OpenOptions,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};
use which::which;

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

impl PathData {
    fn new(path: &Path) -> PathData {
        let absolute_path: PathBuf = if path.is_relative() {
            if let Ok(canonical_path) = path.canonicalize() {
                canonical_path
            } else {
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };

        let (len, time, phantom) = match std::fs::metadata(&absolute_path) {
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
    Deleted,
    Display,
}

#[derive(Debug, Clone, PartialEq)]
enum InteractiveMode {
    None,
    Lookup,
    Select,
    Restore,
}

#[derive(Debug, Clone)]
enum SnapPoint {
    Native(NativeCommands),
    UserDefined(UserDefinedDirs),
}

#[derive(Debug, Clone)]
pub struct UserDefinedDirs {
    snap_dir: PathBuf,
    local_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct NativeCommands {
    zfs_command: String,
    shell_command: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    paths: Vec<PathData>,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live_vers: bool,
    opt_recursive: bool,
    opt_deleted: bool,
    exec_mode: ExecMode,
    snap_point: SnapPoint,
    interactive_mode: InteractiveMode,
    pwd: PathBuf,
    requested_dir: PathData,
    self_command: String,
}

impl Config {
    fn from(
        matches: ArgMatches,
    ) -> Result<Config, Box<dyn std::error::Error + Send + Sync + 'static>> {
        if matches.is_present("ZSH_HOT_KEYS") {
            install_hot_keys()?
        }
        let opt_deleted = matches.is_present("DELETED");
        let opt_zeros = matches.is_present("ZEROS");
        let opt_raw = matches.is_present("RAW");
        let opt_no_pretty = matches.is_present("NOT_SO_PRETTY");
        let opt_no_live_vers = matches.is_present("NO_LIVE");
        let opt_recursive = matches.is_present("RECURSIVE");
        let mut exec_mode = if matches.is_present("INTERACTIVE")
            || matches.is_present("RESTORE")
            || matches.is_present("SELECT")
        {
            ExecMode::Interactive
        } else if opt_deleted {
            ExecMode::Deleted
        } else {
            ExecMode::Display
        };
        let env_snap_dir = std::env::var_os("HTTM_SNAP_POINT");
        let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");
        let interactive_mode = if matches.is_present("RESTORE") {
            InteractiveMode::Restore
        } else if matches.is_present("SELECT") {
            InteractiveMode::Select
        } else if matches.is_present("INTERACTIVE") {
            InteractiveMode::Lookup
        } else {
            InteractiveMode::None
        };

        if opt_recursive && exec_mode == ExecMode::Display {
            return Err(
                HttmError::new("Recursive search feature only allowed in select modes.").into(),
            );
        }

        // need to know the command to execute for previews in interactive mode, best explained why there
        // the 0th element of the env args passed at the command line is the program's path
        let self_command = env::args_os()
            .into_iter()
            .next()
            .ok_or_else(|| HttmError::new("You must place the 'httm' command in your path.  Perhaps the .cargo/bin folder isn't in your path?"))?
            .to_string_lossy()
            .into_owned();

        let pwd = if let Ok(pwd) = std::env::var("PWD") {
            if let Ok(path) = PathBuf::from(&pwd).canonicalize() {
                path
            } else {
                return Err(HttmError::new(
                    "Working directory, as set in your environment, does not appear to exist",
                )
                .into());
            }
        } else {
            return Err(HttmError::new("Working directory is not set in your environment.").into());
        };

        // two ways to get a snap dir: cli and env var
        let raw_snap_var = if let Some(value) = matches.value_of_os("SNAP_POINT") {
            Some(value.to_os_string())
        } else {
            env_snap_dir
        };

        let snap_point = if let Some(raw_value) = raw_snap_var {
            // user defined dir exists?: check that path contains the hidden snapshot directory
            let path = PathBuf::from(raw_value);
            let hidden_snap_dir = path.join(".zfs").join("snapshot");

            let snap_dir = if hidden_snap_dir.metadata().is_ok() {
                path
            } else {
                return Err(HttmError::new(
                    "Manually set mountpoint does not contain a hidden ZFS directory.  Please mount a ZFS directory there or try another mountpoint.",
                ).into());
            };

            // two ways to get a local relative dir: cli and env var
            let raw_local_var = if let Some(raw_value) = matches.value_of_os("LOCAL_DIR") {
                Some(raw_value.to_os_string())
            } else {
                env_local_dir
            };

            // local dir can be set at cmdline or as an env var, but defaults to current working directory
            let local_dir = if let Some(value) = raw_local_var {
                let local_dir: PathBuf = PathBuf::from(value);

                if local_dir.metadata().is_ok() {
                    local_dir
                } else {
                    return Err(HttmError::new(
                        "Manually set local relative directory does not exist.  Please try another.",
                    )
                    .into());
                }
            } else {
                pwd.clone()
            };

            SnapPoint::UserDefined(UserDefinedDirs {
                snap_dir,
                local_dir,
            })
        } else {
            // Make sure we have the necessary commands for execution without a snap point
            let shell_command = which("sh")
                .map_err(|_| {
                    HttmError::new(
                        "sh command not found. Make sure the command 'sh' is in your path.",
                    )
                })?
                .to_string_lossy()
                .into_owned();

            let zfs_command = which("zfs")
                .map_err(|_| {
                    HttmError::new(
                        "zfs command not found. Make sure the command 'zfs' is in your path.",
                    )
                })?
                .to_string_lossy()
                .into_owned();

            SnapPoint::Native(NativeCommands {
                zfs_command,
                shell_command,
            })
        };

        let mut paths: Vec<PathData> = if matches.is_present("INPUT_FILES") {
            // can unwrap because we check if present above
            matches
                .values_of_os("INPUT_FILES")
                .unwrap()
                .into_iter()
                .par_bridge()
                .map(|string| PathData::new(Path::new(string)))
                .collect()
        // setting pwd as the path, here, keeps us from waiting on stdin when in non-Display modes
        } else if exec_mode == ExecMode::Interactive || exec_mode == ExecMode::Deleted {
            vec![PathData::new(&pwd)]
        } else if exec_mode == ExecMode::Display {
            read_stdin()?
                .into_iter()
                .par_bridge()
                .map(|string| PathData::new(Path::new(&string)))
                .collect()
        } else {
            unreachable!()
        };

        // for modes in which we can only take a single directory, process how to handle here
        let requested_dir: PathData = match exec_mode {
            ExecMode::Interactive => {
                match paths.len() {
                    0 => PathData::new(&pwd),
                    1 => {
                        match &paths[0].path_buf {
                            n if n.is_dir() => paths.get(0).unwrap().to_owned(),
                            n if n.is_file() => {
                                match interactive_mode {
                                    InteractiveMode::Lookup | InteractiveMode::None => {
                                        // doesn't make sense to have a non-dir in these modes
                                        return Err(HttmError::new(
                                                "Path specified is not a directory, and therefore not suitable for browsing.",
                                            )
                                            .into());
                                    }
                                    InteractiveMode::Restore | InteractiveMode::Select => {
                                        // non-dir file will just cause us to skip the lookup phase
                                        // this is a value which won't get used
                                        paths.get(0).unwrap().to_owned()
                                    }
                                }
                            }
                            // let's not screw with symlinks, char, block devices, whatever else.
                            _ => {
                                return Err(HttmError::new(
                                    "Path specified is either not a directory or a file, or does not exist, and therefore is not suitable for an interactive mode.",
                                )
                                .into());
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
            ExecMode::Deleted => {
                // paths should never be empty for ExecMode::Deleted
                //
                // we only want one dir for a ExecMode::Deleted run, else
                // we should run in ExecMode::Display mode
                match paths.len() {
                    n if n > 1 => {
                        exec_mode = ExecMode::Display;
                        PathData::new(&pwd)
                    }
                    n if n == 1 => match &paths[0].path_buf {
                        n if n.is_dir() => paths.get(0).unwrap().to_owned(),
                        _ => {
                            exec_mode = ExecMode::Display;
                            PathData::new(&pwd)
                        }
                    },
                    _ => {
                        // paths should never be empty, but here we make sure
                        PathData::new(&pwd)
                    }
                }
            }
            ExecMode::Display => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                PathData::new(&pwd)
            }
        };

        // deduplicate pathdata if in display mode -- so ./.z* and ./.zshrc only print once
        paths = if exec_mode == ExecMode::Display && paths.len() > 1 {
            let mut unique_paths: HashMap<PathBuf, PathData> = HashMap::default();

            paths.into_iter().for_each(|pathdata| {
                let _ = unique_paths.insert(pathdata.path_buf.clone(), pathdata);
            });
            unique_paths.into_iter().map(|(_, v)| v).collect()
        } else {
            paths
        };

        let config = Config {
            paths,
            opt_raw,
            opt_zeros,
            opt_no_pretty,
            opt_no_live_vers,
            opt_recursive,
            opt_deleted,
            snap_point,
            exec_mode,
            interactive_mode,
            pwd,
            requested_dir,
            self_command,
        };

        Ok(config)
    }
}

fn parse_args() -> ArgMatches {
    clap::Command::new("httm")
        .about("\nBy default, httm will display non-interactive information about unique file versions contained on ZFS snapshots.\n\n\
        You may also select from the various interactive modes below to browse for, select, and/or restore files.")
        .version("0.7.0") 
        .arg(
            Arg::new("INPUT_FILES")
                .help("in the default, non-interactive mode, put requested files here.  If you enter no files, \
                then httm will pause waiting for input on stdin(3).  In any interactive mode, this is the search path. \
                If none is entered, httm will use the current working directory.")
                .takes_value(true)
                .multiple_values(true)
                .display_order(1)
        )
        .arg(
            Arg::new("INTERACTIVE")
                .short('i')
                .long("interactive")
                .help("interactively browse files from a fuzzy-finder view.")
                .display_order(2)
        )
        .arg(
            Arg::new("SELECT")
                .short('s')
                .long("select")
                .help("interactively browse files and select snapshot versions from a fuzzy-finder view.")
                .conflicts_with("RESTORE")
                .display_order(3)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .help("interactively browse files and restore from backup from a fuzzy-finder view.")
                .conflicts_with("SELECT")
                .display_order(4)
        )
        .arg(
            Arg::new("DELETED")
                .short('d')
                .long("deleted")
                .help("show deleted files in interactive modes, or do a search for all such files, if a directory is specified.  \
                Note: Any directory listing in interactive mode is slower when enabled.")
                .display_order(5)
        )
        .arg(
            Arg::new("RECURSIVE")
                .short('R')
                .long("recursive")
                .help("recurse into selected directory to find more files. Only available in interactive and deleted file modes.")
                .display_order(6)
        )
        .arg(
            Arg::new("SNAP_POINT")
                .long("snap-point")
                .help("ordinarily httm will automatically choose your most local snapshot directory, \
                but here you may manually specify your own mount point for that directory, such as the mount point for a remote share.  \
                You can also set via the environment variable HTTM_SNAP_POINT.")
                .takes_value(true)
                .display_order(7)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .help("used with SNAP_POINT to determine where the corresponding live root of the ZFS snapshot dataset is.  If not set, \
                httm defaults to your current working directory.  You can also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("SNAP_POINT")
                .takes_value(true)
                .display_order(8)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .help("display the backup locations only, without extraneous information, delimited by a NEWLINE.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"])
                .display_order(9)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("display the backup locations only, without extraneous information, delimited by a NULL CHARACTER.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"])
                .display_order(10)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(11)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .help("only display information concerning snapshot versions, and no 'live' versions of files or directories.")
                .display_order(12)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(12)
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
        // 2. Get PathData struct for all paths - lens, modify times, paths
        // 3. Determine/lookup whether file matches any files on snapshots
        ExecMode::Interactive => lookup_exec(&config, &interactive_exec(&mut out, &config)?)?,
        ExecMode::Display => lookup_exec(&config, &config.paths)?,
        // deleted_exec is special because it is more convenient to get PathData in 'mod deleted'
        // on raw paths rather than strings, also there is no need to run a lookup on files already on snapshots
        ExecMode::Deleted => deleted_exec(&config, &mut out)?,
    };

    // and display
    let output_buf = display_exec(&config, snaps_and_live_set)?;

    write!(out, "{}", output_buf)?;
    out.flush()?;

    Ok(())
}

fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    stdin.read_line(&mut buffer)?;

    let broken_string: Vec<String> = buffer
        .split_ascii_whitespace()
        .into_iter()
        .map(|i| i.to_owned())
        .collect();

    Ok(broken_string)
}

fn install_hot_keys() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get our home directory
    let home_dir = if let Ok(home) = std::env::var("HOME") {
        if let Ok(path) = PathBuf::from(&home).canonicalize() {
            path
        } else {
            return Err(
                HttmError::new("$HOME does not appear to be set in your environment").into(),
            );
        }
    } else {
        return Err(
            HttmError::new("$HOME, as set in your environment, does not appear to exist").into(),
        );
    };

    // check whether httm-key-bindings.zsh is already sourced
    // and open ~/.zshrc for later
    let mut buffer = String::new();
    let zshrc_path: PathBuf = [&home_dir, &PathBuf::from(".zshrc")].iter().collect();
    let mut zshrc_file = if let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .append(true)
        .open(zshrc_path)
    {
        file
    } else {
        return Err(HttmError::new(
                "Either your ~/.zshrc file does not exist or you do not have the permissions to access it.",
            )
            .into());
    };
    zshrc_file.read_to_string(&mut buffer)?;

    // check that there are not lines in the zshrc that contain "source" and "httm-key-bindings.zsh"
    if !buffer
        .lines()
        .filter(|line| !line.starts_with('#'))
        .any(|line| line.contains("source") && line.contains("httm-key-bindings.zsh"))
    {
        // create key binding file -- done at compile time
        let zsh_hot_key_script = include_str!("../scripts/httm-key-bindings.zsh");
        let zsh_script_path: PathBuf = [&home_dir, &PathBuf::from(".httm-key-bindings.zsh")]
            .iter()
            .collect();
        // creates script file in user's home dir or will fail if file already exists
        if let Ok(mut zsh_script_file) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(zsh_script_path)
        {
            zsh_script_file.write_all(zsh_hot_key_script.as_bytes())?;
        } else {
            eprintln!("httm: httm-key-bindings.zsh is already present in user's home directory.");
        }

        // append "source ~/.httm-key-bindings.zsh" to zshrc
        zshrc_file.write_all(
            "\n# httm: zsh hot keys script\nsource ~/.httm-key-bindings.zsh\n".as_bytes(),
        )?;
        eprintln!("httm: zsh hot keys were installed successfully.");
    } else {
        eprintln!(
            "httm: zsh hot keys appear to already be sourced in the user's ~/.zshrc. Quitting."
        );
    }

    std::process::exit(0)
}
