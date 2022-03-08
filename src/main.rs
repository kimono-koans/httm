// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use chrono::{DateTime, Local};
use clap::{Arg, ArgMatches};
use fxhash::FxHashMap as HashMap;
use number_prefix::NumberPrefix;
use std::{
    error::Error,
    ffi::OsString,
    fmt,
    fs::canonicalize,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Command as ExecProcess,
    time::SystemTime,
};

#[derive(Debug)]
struct HttmError {
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
struct PathData {
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

        // call for canonical path won't work unless path is dir.
        // Also, safe to unwrap as we check invariants above,
        // but let's keep living safely
        let mut canonical_parent = match canonicalize(parent) {
            Ok(cp) => cp,
            Err(_) => {
                if path.is_relative() {
                    if let Ok(pwd) = std::env::var("PWD") {
                        PathBuf::from(&pwd)
                    } else {
                        PathBuf::from("/")
                    }
                } else {
                    PathBuf::from("/")
                }
            }
        };

        // add last component to parent path
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
            // because we need to account for any phantom files later
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

struct Config {
    path_data: Vec<Option<PathData>>,
    opt_raw: bool,
    opt_zeros: bool,
    opt_no_pretty: bool,
    opt_no_live_vers: bool,
    opt_man_mnt_point: Option<OsString>,
    working_dir: PathBuf,
}

impl Config {
    fn from(matches: ArgMatches) -> Result<Config, Box<dyn std::error::Error>> {
        let zeros = matches.is_present("ZEROS");
        let raw = matches.is_present("RAW");
        let no_so_pretty = matches.is_present("NOT_SO_PRETTY");
        let no_live_vers = matches.is_present("NO_LIVE");

        let pwd = if let Ok(pwd) = std::env::var("PWD") {
            PathBuf::from(&pwd)
        } else {
            PathBuf::from("/")
        };

        let mnt_point = if let Some(raw_value) = matches.value_of_os("MANUAL_MNT_POINT") {
            // very path contains hidden snapshot directory
            let mut snapshot_dir: PathBuf = PathBuf::from(&raw_value);
            snapshot_dir.push(".zfs");
            snapshot_dir.push("snapshot");

            if snapshot_dir.metadata().is_ok() {
                Some(raw_value.to_os_string())
            } else {
                return Err(HttmError::new(
                    "Manually set mountpoint does not contain a hidden ZFS directory.  Please try another.",
                ).into());
            }
        } else {
            None
        };

        let file_names: Vec<String> = if matches.is_present("INPUT_FILES") {
            let raw_values = matches.values_of_os("INPUT_FILES").unwrap();

            let mut res = Vec::new();

            for i in raw_values {
                if let Ok(r) = i.to_owned().into_string() {
                    res.push(r);
                }
            }

            if res.get(0).unwrap() == &String::from("-") {
                read_stdin()?
            } else {
                res
            }
        } else {
            read_stdin()?
        };

        let mut vec_pd: Vec<Option<PathData>> = Vec::new();

        for file in file_names {
            let path = Path::new(&file);
            if path.is_relative() {
                let mut pwd_clone = pwd.clone();
                pwd_clone.push(path);
                vec_pd.push(PathData::new(&pwd_clone))
            } else {
                vec_pd.push(PathData::new(path))
            }
        }

        let config = Config {
            path_data: vec_pd,
            opt_raw: raw,
            opt_zeros: zeros,
            opt_no_pretty: no_so_pretty,
            opt_no_live_vers: no_live_vers,
            opt_man_mnt_point: mnt_point,
            working_dir: pwd,
        };

        Ok(config)
    }
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

    let matches = clap::Command::new("httm").about("is *not* an acronym for Hot Tub Time Machine")
        .arg(
            Arg::new("INPUT_FILES")
                .help("...you should put your files here, if stdin(3) is not your flavor.")
                .takes_value(true)
                .multiple_values(true)
        )
        .arg(
            Arg::new("MANUAL_MNT_POINT")
                .short('m')
                .long("mnt-point")
                .help("ordinary httm automatically uses your local  manually specify your mount point")
                .takes_value(true)
        )
        .arg(
            Arg::new("RAW")
                .short('r')
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
        .get_matches();

    let config = Config::from(matches)?;

    let mut snapshot_versions: Vec<PathData> = Vec::new();

    for instance_pd in config.path_data.iter().flatten() {
        let dataset = if let Some(ref mp) = config.opt_man_mnt_point {
            mp.to_owned()
        } else {
            get_dataset(instance_pd)?
        };

        snapshot_versions.extend_from_slice(&get_versions(&config, instance_pd, dataset)?);
    }

    let mut live_versions: Vec<PathData> = Vec::new();

    if !config.opt_no_live_vers {
        for instance_pd in &config.path_data {
            if instance_pd.is_some() {
                live_versions.push(instance_pd.clone().unwrap());
            }
        }
    }

    if snapshot_versions.is_empty() || (live_versions.len() == 1 && live_versions[0].is_phantom) {
        return Err(HttmError::new(
            "File does not seem to exist, umm, 🤷? Please try another file.",
        )
        .into());
    }

    let working_set: Vec<Vec<PathData>> = if config.opt_no_live_vers {
        vec![snapshot_versions]
    } else {
        vec![snapshot_versions, live_versions]
    };

    if config.opt_raw || config.opt_zeros {
        display_raw(&mut out, &config, working_set)?
    } else {
        display_pretty(&mut out, &config, working_set)?
    }

    out.flush()?;

    Ok(())
}

fn display_raw(
    out: &mut std::io::Stdout,
    config: &Config,
    working_set: Vec<Vec<PathData>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    for version in &working_set {
        for pd in version {
            let d_path = pd.path_buf.display().to_string();
            write!(out, "{}", d_path)?;
            write!(out, "{}", delimiter)?;
        }
    }

    Ok(())
}

fn display_pretty(
    out: &mut std::io::Stdout,
    config: &Config,
    working_set: Vec<Vec<PathData>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut size_padding = 1usize;
    let mut fancy_border = 1usize;

    // calculate padding and borders
    for version in &working_set {
        for pd in version {
            let d_date = display_date(&pd.system_time);
            let d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let fixed_padding = format!("{:<5}", " ");
            let d_path = pd.path_buf.display().to_string();

            let d_size_len = display_human_size(pd).len();
            let formatted_line_len =
                d_date.len() + d_size.len() + fixed_padding.len() + d_path.len() + 2usize;

            size_padding = d_size_len.max(size_padding);
            fancy_border = formatted_line_len.max(fancy_border);
        }
    }

    size_padding += 5usize;
    fancy_border += 5usize;

    let fancy_string: String = {
        let mut res: String = String::new();
        for _ in 0..fancy_border {
            res += "-";
        }
        res
    };

    // display all
    if !config.opt_no_pretty {
        writeln!(out, "{}", fancy_string)?;
    }
    let mut buffer = String::new();
    for version in &working_set {
        for pd in version {
            let d_date = display_date(&pd.system_time);
            let mut d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let mut fixed_padding = format!("{:<5}", " ");
            let d_path = pd.path_buf.display();

            if config.opt_no_pretty {
                fixed_padding = "\t".to_owned();
                d_size = format!("\t{}", display_human_size(pd));
            };

            if !pd.is_phantom {
                buffer += &format!("{}{}{}\"{}\"\n", d_date, d_size, fixed_padding, d_path);
            } else {
                let mut pad_date: String = String::new();
                let mut pad_size: String = String::new();
                for _ in 0..d_date.len() {
                    pad_date += " ";
                }
                for _ in 0..d_size.len() {
                    pad_size += " ";
                }
                buffer += &format!("{}{}{}\"{}\"\n", pad_date, pad_size, fixed_padding, d_path);
            }
        }
        if !config.opt_no_pretty {
            buffer += &format!("{}\n", fancy_string);
        }
    }

    if config.opt_no_pretty {
        for line in buffer.lines().rev() {
            writeln!(out, "{}", line)?
        }
    } else {
        write!(out, "{}", buffer)?
    };

    Ok(())
}

fn display_human_size(pd: &PathData) -> String {
    let size = pd.size as f64;

    match NumberPrefix::binary(size) {
        NumberPrefix::Standalone(bytes) => {
            format!("{} bytes", bytes)
        }
        NumberPrefix::Prefixed(prefix, n) => {
            format!("{:.1} {}B", n, prefix)
        }
    }
}

fn display_date(st: &SystemTime) -> String {
    let dt: DateTime<Local> = st.to_owned().into();
    format!("{}", dt.format("%b %e %H:%M:%S %Y"))
}

fn get_versions(
    config: &Config,
    pathdata: &PathData,
    dataset: OsString,
) -> Result<Vec<PathData>, Box<dyn std::error::Error>> {
    let mut snapshot_dir: PathBuf = PathBuf::from(&dataset);
    snapshot_dir.push(".zfs");
    snapshot_dir.push("snapshot");

    let relative_path = if config.opt_man_mnt_point.is_some() {
        pathdata.path_buf.strip_prefix(&config.working_dir)?
    } else {
        pathdata.path_buf.strip_prefix(&dataset)?
    };

    let snapshots = std::fs::read_dir(snapshot_dir)?;

    let versions: Vec<_> = snapshots
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .map(|path| path.join(relative_path))
        .collect();

    let mut unique_versions: HashMap<(SystemTime, u64), PathData> = HashMap::default();

    for path in &versions {
        if let Some(pd) = PathData::new(path) {
            if !pd.is_phantom {
                unique_versions.insert((pd.system_time, pd.size), pd);
            }
        }
    }

    let mut sorted: Vec<_> = unique_versions.into_iter().collect();

    sorted.sort_by_key(|&(k, _)| k);

    Ok(sorted.into_iter().map(|(_, v)| v).collect())
}

fn get_dataset(pathdata: &PathData) -> Result<OsString, Box<dyn std::error::Error>> {
    let path = &pathdata.path_buf;

    // fn parent() cannot return None, when path is a canonical path
    let parent_folder = path.parent().unwrap_or(path).to_string_lossy();

    let exec_command = "zfs list -H -t filesystem -o mountpoint,mounted";

    let raw_ds = std::str::from_utf8(
        &ExecProcess::new("env")
            .arg("sh")
            .arg("-c")
            .arg(exec_command)
            .output()?
            .stdout,
    )?
    .to_owned();

    let select_potential_mountpoints = raw_ds
        .lines()
        .filter(|line| line.contains("yes"))
        .filter_map(|line| line.split('\t').next())
        .map(|line| line.trim())
        .filter(|line| parent_folder.contains(line))
        .collect::<Vec<&str>>();

    if select_potential_mountpoints.is_empty() {
        let msg = "Could not identify any qualifying dataset.  Maybe consider specifying manually?"
            .to_string();
        return Err(HttmError::new(&msg).into());
    };

    let best_potential_mountpoint = if let Some(bpmp) = select_potential_mountpoints
        .clone()
        .into_iter()
        .max_by_key(|x| x.len())
    {
        bpmp
    } else {
        let msg = format!(
            "There is no best match for a ZFS dataset to use for path {:?}. Sorry!/Not sorry?)",
            path
        );
        return Err(HttmError::new(&msg).into());
    };

    Ok(OsString::from(best_potential_mountpoint))
}
