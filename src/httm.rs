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

mod display;
mod interactive;
mod lookup;

use crate::display::display_exec;
use crate::interactive::interactive_exec;
use crate::lookup::lookup_exec;

use clap::{Arg, ArgMatches};
use std::{
    error::Error,
    fmt,
    io::{BufRead, Write},
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

#[derive(Clone)]
pub struct PathData {
    system_time: SystemTime,
    size: u64,
    path_buf: PathBuf,
    is_phantom: bool,
}

impl PathData {
    fn new(config: &Config, path: &Path) -> PathData {
        let absolute_path: PathBuf = if path.is_relative() {
            [
                PathBuf::from(&config.current_working_dir),
                path.to_path_buf(),
            ]
            .iter()
            .collect()
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
            // however we will want certain iters to print the *request*, say for deleted/fake files,
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
pub struct Config {
    raw_paths: Vec<String>,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live_vers: bool,
    opt_recursive: bool,
    opt_snap_point: Option<PathBuf>,
    opt_local_dir: PathBuf,
    exec_mode: ExecMode,
    interactive_mode: InteractiveMode,
    current_working_dir: PathBuf,
    user_requested_dir: PathBuf,
}

impl Config {
    fn from(matches: ArgMatches) -> Result<Config, Box<dyn std::error::Error>> {
        let zeros = matches.is_present("ZEROS");
        let raw = matches.is_present("RAW");
        let not_so_pretty = matches.is_present("NOT_SO_PRETTY");
        let no_live_vers = matches.is_present("NO_LIVE");
        let exec = if matches.is_present("INTERACTIVE")
            || matches.is_present("RESTORE")
            || matches.is_present("SELECT")
        {
            ExecMode::Interactive
        } else {
            ExecMode::Display
        };
        let env_snap_dir = std::env::var_os("HTTM_SNAP_POINT");
        let env_local_dir = std::env::var_os("HTTM_LOCAL_DIR");
        let recursive = matches.is_present("RECURSIVE");
        let interactive = if matches.is_present("RESTORE") {
            InteractiveMode::Restore
        } else if matches.is_present("SELECT") {
            InteractiveMode::Select
        } else if matches.is_present("INTERACTIVE") {
            InteractiveMode::Lookup
        } else {
            InteractiveMode::None
        };

        if recursive && interactive == InteractiveMode::None {
            return Err(HttmError::new(
                "Recursive search feature only allowed in one of the interactive modes.",
            )
            .into());
        }

        // two ways to get a snap dir: cli and env var
        let raw_snap_var = if let Some(value) = matches.value_of_os("SNAP_POINT") {
            Some(value.to_os_string())
        } else {
            env_snap_dir
        };

        let snap_point = if let Some(raw_value) = raw_snap_var {
            // dir exists sanity check?: check that path contains the hidden snapshot directory
            let path = PathBuf::from(raw_value);
            let snapshot_dir = path.join(".zfs").join("snapshot");

            if snapshot_dir.metadata().is_ok() {
                Some(path)
            } else {
                return Err(HttmError::new(
                    "Manually set mountpoint does not contain a hidden ZFS directory.  Please mount a ZFS directory there or try another mountpoint.",
                ).into());
            }
        } else {
            None
        };

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

        // two ways to get a local relative dir: cli and env var
        let raw_local_var = if let Some(raw_value) = matches.value_of_os("LOCAL_DIR") {
            Some(raw_value.to_os_string())
        } else {
            env_local_dir
        };

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

        let file_names: Vec<String> = if matches.is_present("INPUT_FILES") {
            let raw_values = matches.values_of_os("INPUT_FILES").unwrap();
            raw_values
                .map(|i| i.to_string_lossy().into_owned())
                .collect()
        } else if exec == ExecMode::Interactive {
            Vec::new()
        } else {
            read_stdin()?
        };

        let requested_dir = if exec == ExecMode::Interactive {
            if file_names.len() > 1usize {
                return Err(
                    HttmError::new("May only specify one path in interactive mode.").into(),
                );
            } else if file_names.len() == 1 && !Path::new(&file_names[0]).is_dir() {
                return Err(HttmError::new(
                    "Path specified is not a directory suitable for browsing.",
                )
                .into());
            } else if file_names.len() == 1 && PathBuf::from(&file_names[0]).is_dir() {
                PathBuf::from(&file_names.get(0).unwrap()).canonicalize()?
            } else if file_names.is_empty() {
                pwd.clone()
            } else {
                unreachable!()
            }
        } else {
            // in non-interactive mode / display mode, requested dir is just a file
            // like every other file and pwd must be the requested working dir.
            pwd.clone()
        };

        let config = Config {
            raw_paths: file_names,
            opt_raw: raw,
            opt_zeros: zeros,
            opt_no_pretty: not_so_pretty,
            opt_no_live_vers: no_live_vers,
            opt_snap_point: snap_point,
            opt_local_dir: local_dir,
            opt_recursive: recursive,
            exec_mode: exec,
            interactive_mode: interactive,
            current_working_dir: pwd,
            user_requested_dir: requested_dir,
        };

        Ok(config)
    }
}

fn parse_args() -> ArgMatches {
    clap::Command::new("httm")
        .about("displays information about unique file versions contained on ZFS snapshots.\n\n*But don't call it a H__ T__ Time Machine.*")
        .version("0.5.2") 
        .arg(
            Arg::new("INPUT_FILES")
                .help("in non-interactive mode, put requested files here.  In interactive mode, this is the search path.  If you enter no files, then httm will pause waiting for input on stdin(3).")
                .takes_value(true)
                .multiple_values(true)
                .display_order(1)
        )
        .arg(
            Arg::new("INTERACTIVE")
                .short('i')
                .long("interactive")
                .help("use native dialogs for an interactive lookup session.")
                .multiple_values(true)
                .display_order(2)
        )
        .arg(
            Arg::new("SELECT")
                .short('s')
                .long("select")
                .help("use native dialogs for an interactive lookup and file select.")
                .multiple_values(true)
                .display_order(3)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .help("use native dialogs for an interactive restore from backup.")
                .multiple_values(true)
                .display_order(4)
        )
        .arg(
            Arg::new("RECURSIVE")
                .short('R')
                .long("recursive")
                .help("recurse into selected directory to find more files. Only available in interactive modes.")
                .display_order(5)
        )
        .arg(
            Arg::new("SNAP_POINT")
                .long("snap-point")
                .help("ordinarily httm will automatically choose your most local snapshot directory, but here you may manually specify your own mount point for that directory, such as the mount point for a remote share.  You can also set via the environment variable HTTM_SNAP_POINT.")
                .takes_value(true)
                .display_order(6)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .help("used with SNAP_POINT to determine where the corresponding live root of the ZFS snapshot dataset is.  If not set, httm defaults to your current working directory.  You can also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("SNAP_POINT")
                .takes_value(true)
                .display_order(7)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .help("list the backup locations, without extraneous information, delimited by a NEWLINE.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"])
                .display_order(8)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("list the backup locations, without extraneous information, delimited by a NULL CHARACTER.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"])
                .display_order(9)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .help("list the backup locations in a parseable format.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(10)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .help("only display snapshot copies, and no 'live' copies of files or directories.")
                .display_order(11)
        )
        .get_matches()
}

fn main() {
    if let Err(e) = exec() {
        eprintln!("Error: {}", e);
        std::process::exit(1)
    } else {
        std::process::exit(0)
    }
}

fn exec() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout();

    // get our program args and generate a config for use
    // everywhere else
    let arg_matches = parse_args();
    let config = Config::from(arg_matches)?;

    // next, let's do our interactive lookup thing, if appropriate,
    // and for all relevant strings get our PathData struct
    let pathdata_set = if config.exec_mode == ExecMode::Interactive {
        get_pathdata(&config, &interactive_exec(&mut out, &config)?)?
    } else {
        get_pathdata(&config, &config.raw_paths)?
    };

    // finally run search on those paths
    let snaps_and_live_set = lookup_exec(&config, pathdata_set)?;

    // and display
    let output_buf = display_exec(&config, snaps_and_live_set)?;

    write!(out, "{}", output_buf)?;
    out.flush()?;

    Ok(())
}

fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error>> {
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

pub fn get_pathdata(
    config: &Config,
    paths_as_strings: &[String],
) -> Result<Vec<PathData>, Box<dyn std::error::Error>> {
    // build our pathdata Vecs for our lookup request
    let vec_pd: Vec<PathData> = paths_as_strings
        .iter()
        .map(|string| PathData::new(config, Path::new(&string)))
        .collect();
    Ok(vec_pd)
}
