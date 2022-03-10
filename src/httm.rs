// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use clap::{Arg, ArgMatches};
use std::io::BufRead;
use std::time::SystemTime;
use std::{
    error::Error,
    ffi::OsString,
    fmt,
    fs::canonicalize,
    io::Write,
    path::{Path, PathBuf},
};

mod interactive;
use crate::interactive::*;
mod lookup;
use crate::lookup::*;
mod display;
use crate::display::*;

#[derive(Debug)]
pub struct HttmError {
    details: String,
}

impl HttmError {
    fn new(msg: &str) -> HttmError {
        HttmError {
            details: msg.to_string(),
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
    fn new(path: &Path) -> Option<PathData> {
        let parent = if let Some(parent) = path.parent() {
            parent
        } else {
            Path::new("/")
        };

        let mut canonical_parent = if path.is_relative() {
            if let Ok(pwd) = std::env::var("PWD") {
                PathBuf::from(&pwd)
            } else {
                PathBuf::from("/")
            }
        } else if let Ok(cp) = canonicalize(parent) {
            cp
        } else {
            PathBuf::from("/")
        };

        // add last component, filename, to parent path
        canonical_parent.push(path);
        let absolute_path = canonical_parent;

        let len;
        let time;
        let phantom;

        match std::fs::metadata(&absolute_path) {
            Ok(md) => {
                len = md.len();
                time = md.modified().ok()?;
                phantom = false;
            }
            // this seems like a perfect place for a None value, as the file has no metadata,
            // however we will want certain iters to print the *request*, say for deleted/fake files,
            // so we set up a dummy Some value just so we can have the path names we entered
            //
            // if we get a spurious example of no metadata in snapshot directories we just ignore later
            Err(_) => {
                len = 0u64;
                time = SystemTime::UNIX_EPOCH;
                phantom = true;
            }
        }

        Some(PathData {
            system_time: time,
            size: len,
            path_buf: absolute_path,
            is_phantom: phantom,
        })
    }
}

pub struct Config {
    raw_paths: Vec<String>,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live_vers: bool,
    opt_interactive: bool,
    opt_restore: bool,
    opt_man_mnt_point: Option<OsString>,
    opt_relative_dir: Option<OsString>,
    current_working_dir: PathBuf,
    user_requested_dir: PathBuf,
}

impl Config {
    fn from(matches: ArgMatches) -> Result<Config, Box<dyn std::error::Error>> {
        let zeros = matches.is_present("ZEROS");
        let raw = matches.is_present("RAW");
        let no_so_pretty = matches.is_present("NOT_SO_PRETTY");
        let no_live_vers = matches.is_present("NO_LIVE");
        let interactive = matches.is_present("INTERACTIVE") || matches.is_present("RESTORE");
        let restore = matches.is_present("RESTORE");

        let mnt_point = if let Some(raw_value) = matches.value_of_os("MANUAL_MNT_POINT") {
            // dir exists sanity check?: check that path contains the hidden snapshot directory
            let mut snapshot_dir: PathBuf = PathBuf::from(&raw_value);
            snapshot_dir.push(".zfs");
            snapshot_dir.push("snapshot");

            if snapshot_dir.metadata().is_ok() {
                Some(raw_value.to_os_string())
            } else {
                return Err(HttmError::new(
                    "Manually set mountpoint does not contain a hidden ZFS directory.  Please mount a ZFS directory there or try another mountpoint.",
                ).into());
            }
        } else {
            None
        };

        let relative_dir = if let Some(raw_value) = matches.value_of_os("RELATIVE_DIR") {
            // dir exists sanity check?: check path exists by checking for path metadata
            if PathBuf::from(raw_value).metadata().is_ok() {
                Some(raw_value.to_os_string())
            } else {
                return Err(HttmError::new(
                    "Manually set relative directory does not exist.  Please try another.",
                )
                .into());
            }
        } else {
            None
        };

        // working dir from env
        let pwd = if let Ok(pwd) = std::env::var("PWD") {
            PathBuf::from(&pwd)
        } else {
            PathBuf::from("/")
        };

        let file_names: Vec<String> = if matches.is_present("INPUT_FILES") {
            let raw_values = matches.values_of_os("INPUT_FILES").unwrap();

            let mut res = Vec::new();

            for i in raw_values {
                if let Ok(r) = i.to_owned().into_string() {
                    res.push(r);
                }
            }
            res
        } else if interactive {
            Vec::new()
        } else {
            read_stdin()?
        };

        // is there a user defined working dir given at the cli?
        let requested_dir = if interactive
            && file_names.get(0).is_some()
            && PathBuf::from(file_names.get(0).unwrap()).is_dir()
        {
            PathBuf::from(&file_names.get(0).unwrap())
        } else {
            pwd.clone()
        };

        let config = Config {
            raw_paths: file_names,
            opt_raw: raw,
            opt_zeros: zeros,
            opt_no_pretty: no_so_pretty,
            opt_no_live_vers: no_live_vers,
            opt_man_mnt_point: mnt_point,
            current_working_dir: pwd,
            opt_relative_dir: relative_dir,
            opt_interactive: interactive,
            opt_restore: restore,
            user_requested_dir: requested_dir,
        };

        Ok(config)
    }
}

fn parse_args() -> ArgMatches {
    clap::Command::new("httm").about("displays information about unique file versions contained on ZFS snapshots.\n\n *** Now with interactive file selections!! *** \n\n*Not* an acronym for Hot Tub Time Machine?")
        .arg(
            Arg::new("INPUT_FILES")
                .help("...you should put your files here, if stdin(3) is not your flavor.")
                .takes_value(true)
                .multiple_values(true)
        )
        .arg(
            Arg::new("INTERACTIVE")
                .short('i')
                .long("interactive")
                .help("use native dialogs for an interactive lookup session.")
                .multiple_values(true)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .help("use native dialogs for an interactive restore from backup.")
                .multiple_values(true)
        )
        .arg(
            Arg::new("MANUAL_MNT_POINT")
                .long("mnt-point")
                .help("ordinarily httm will automatically choose your most local snapshot directory, but here you may manually specify your own mount point.")
                .takes_value(true),
        )
        .arg(
            Arg::new("RELATIVE_DIR")
                .long("relative")
                .help("used with MANUAL_MNT_POINT to determine where to the corresponding live root of the ZFS snapshot dataset is.  If not set, httm defaults to the working directory.")
                .requires("MANUAL_MNT_POINT")
                .takes_value(true),
        )
        .arg(
            Arg::new("RAW")
                .long("raw")
                .help("list the backup locations, without extraneous information, delimited by a NEWLINE.")
                .conflicts_with_all(&["ZEROS", "NOT_SO_PRETTY"]),
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .help("list the backup locations, without extraneous information, delimited by a NULL CHARACTER.")
                .conflicts_with_all(&["RAW", "NOT_SO_PRETTY"]),
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .help("list the backup locations in a parseable format.")
                .conflicts_with_all(&["RAW", "ZEROS"]),
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .help("only display snapshot copies, and no 'live' copies of files or directories."),
        )
        .get_matches()
}

// happy 'lil main fn that only handles errors
fn main() {
    if let Err(e) = exec() {
        eprintln!("error {}", e);
        std::process::exit(1)
    } else {
        std::process::exit(0)
    }
}

fn exec() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout();
    let arg_matches = parse_args();
    let config = Config::from(arg_matches)?;

    // next, let's do our interactive lookup thing, if appropriate
    // and modify strings returned according to the interactive session
    let raw_paths = interactive_exec(&mut out, &config, &config.user_requested_dir)?;

    // build pathdata from strings
    let pathdata_set = convert_strings_to_pathdata(&config, &raw_paths)?;

    // finally run search on those paths
    let working_set = run_search(&config, pathdata_set)?;

    // and display
    let output_buf = if config.opt_raw || config.opt_zeros {
        display_raw(&config, working_set)?
    } else {
        display_pretty(&config, working_set)?
    };

    write!(out, "{}", output_buf)?;
    out.flush()?;

    Ok(())
}

fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    stdin.read_line(&mut buffer)?;

    let mut broken_string: Vec<String> = Vec::new();

    for i in buffer.split_ascii_whitespace() {
        broken_string.push(i.to_owned())
    }

    Ok(broken_string)
}

pub fn convert_strings_to_pathdata(
    config: &Config,
    raw_paths: &[String],
) -> Result<Vec<Option<PathData>>, Box<dyn std::error::Error>> {
    // build our pathdata Vecs for our lookup request
    let mut vec_pd: Vec<Option<PathData>> = Vec::new();

    for string in raw_paths {
        let path = Path::new(&string);
        if path.is_relative() {
            let mut wd = config.user_requested_dir.clone();
            wd.push(path);
            vec_pd.push(PathData::new(&wd))
        } else {
            vec_pd.push(PathData::new(path))
        }
    }

    Ok(vec_pd)
}
