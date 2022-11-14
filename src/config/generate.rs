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

use std::fs::canonicalize;
use std::path::{Path, PathBuf};

use clap::OsValues;
use rayon::prelude::*;

use clap::{crate_name, crate_version, Arg, ArgMatches};
use indicatif::ProgressBar;
use time::UtcOffset;

use crate::config::install_hot_keys::install_hot_keys;
use crate::data::filesystem_info::FilesystemInfo;
use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{httm_is_dir, read_stdin};
use crate::ROOT_DIRECTORY;

#[derive(Debug, Clone)]
pub enum ExecMode {
    Interactive(InteractiveMode),
    DisplayRecursive(indicatif::ProgressBar),
    Display,
    SnapFileMount(String),
    MountsForFiles,
    NumVersions(NumVersionsMode),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractiveMode {
    Browse,
    Select,
    Restore,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeletedMode {
    DepthOfOne,
    Enabled,
    Only,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NumVersionsMode {
    All,
    SingleAll,
    SingleNoSnap,
    SingleWithSnap,
    Multiple,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LastSnapMode {
    Any,
    None,
    DittoOnly,
    NoDittoExclusive,
    NoDittoInclusive,
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
            Arg::new("BROWSE")
                .short('b')
                .short_alias('i')
                .long("browse")
                .visible_alias("interactive")
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
            Arg::new("PREVIEW")
                .long("preview")
                .help("user may specify a command to preview snapshots while in select view.  The default command is a 'bowie' formatted 'diff'. ")
                .takes_value(true)
                .min_values(0)
                .require_equals(true)
                .default_missing_value("default")
                .display_order(8)
        )
        .arg(
            Arg::new("EXACT")
                .short('e')
                .long("exact")
                .help("use exact pattern matching for searches in the interactive modes (in contrast to the default fuzzy-finder searching).")
                .display_order(9)
        )
        .arg(
            Arg::new("SNAP_FILE_MOUNT")
                .short('S')
                .long("snap")
                .takes_value(true)
                .min_values(0)
                .require_equals(true)
                .default_missing_value("httmSnapFileMount")
                .visible_aliases(&["snap-file", "snapshot", "snap-file-mount"])
                .help("snapshot the mount point/s of the dataset/s which contains the input file/s.  \
                This argument takes a value for an optional snapshot suffix.  The default suffix is 'httmSnapFileMount'.  \
                Note: This is a ZFS only option.")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE", "ALT_REPLICATED", "SNAP_POINT", "LOCAL_DIR"])
                .display_order(10)
        )
        .arg(
            Arg::new("MOUNT_FOR_FILE")
                .short('m')
                .long("mount-for-file")
                .visible_alias("mount")
                .help("display the mount point/s of the dataset/s which contains the input file/s.")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE"])
                .display_order(11)
        )
        .arg(
            Arg::new("LAST_SNAP")
                .short('l')
                .long("last-snap")
                .takes_value(true)
                .default_missing_value("any")
                .possible_values(&["any", "ditto", "no-ditto", "no-ditto-exclusive", "no-ditto-inclusive", "none"])
                .min_values(0)
                .require_equals(true)
                .help("automatically select and print the path of last-in-time unique snapshot version for the input file.  \
                Possible options are: \
                \"any\", return the last in time snapshot version, this is the default, \
                \"ditto\", return only last snaps which are the same as the live file version, \
                \"no-ditto-exclusive\", return only a last snap which is not the same as the live version (\"not ditto\" is an alias for this option), \
                \"no-ditto-inclusive\", return a last snap which is not the same as the live version, \
                or should non-exist, return the live file, and, \
                \"none\", return the live file only for those files without a last snapshot.")
                .conflicts_with_all(&["NUM_VERSIONS", "SNAP_FILE_MOUNT", "MOUNT_FOR_FILE", "ALT_REPLICATED", "SNAP_POINT", "LOCAL_DIR"])
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
            Arg::new("NO_FILTER")
                .long("no-filter")
                .help("by default, in the interactive modes, httm will filter out results from non-supported datasets (like ext4, tmpfs, procfs, sysfs, or devtmpfs), and in common snapshot paths.  \
                Here, one may select to disable such filtering.  httm, however, should always show the input path, and results from behind any input path when that path is searched.")
                .display_order(15)
        )
        .arg(
            Arg::new("NO_TRAVERSE")
                .long("no-traverse")
                .help("in recursive mode, don't traverse symlinks.  Although httm does its best to prevent searching pathologically recursive symlink-ed paths, \
                here, you may disable symlink traversal completely.  NOTE: httm will never traverse symlinks when a requested recursive search is on the root/base directory (\"/\").")
                .display_order(16)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .visible_aliases(&["tabs", "plain-jane"])
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS"])
                .display_order(17)
        )
        .arg(
            Arg::new("OMIT_DITTO")
                .long("omit-ditto")
                .help("omit display of the snapshot version which may be identical to the live version (`httm` ordinarily displays *all* snapshot versions and the live version).")
                .conflicts_with_all(&["NUM_VERSIONS"])
                .display_order(18)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .visible_aliases(&["dead", "disco"])
                .help("only display information concerning snapshot versions (display no information regarding 'live' versions of files or directories).")
                .display_order(19)
        )
        .arg(
            Arg::new("NO_SNAP")
                .long("no-snap")
                .visible_aliases(&["undead", "zombie"])
                .help("only display information concerning 'pseudo-live' versions in Display Recursive mode (in --deleted, --recursive, but non-interactive modes).  \
                Useful for finding the \"files that once were\" and displaying only those pseudo-live/undead files.")
                .requires("RECURSIVE")
                .conflicts_with_all(&["INTERACTIVE", "SELECT", "RESTORE", "SNAP_FILE_MOUNT", "LAST_SNAP", "NOT_SO_PRETTY"])
                .display_order(20)
        )
        .arg(
            Arg::new("MAP_ALIASES")
                .long("map-aliases")
                .visible_aliases(&["aliases"])
                .help("manually map a local directory (eg. \"/Users/<User Name>\") as an alias of a mount point for ZFS or btrfs, \
                such as the local mount point for a backup on a remote share (eg. \"/Volumes/Home\").  \
                This option is useful if you wish to view snapshot versions from within the local directory you back up to your remote share.  \
                Such map is delimited by a colon, ':', and specified as <LOCAL_DIR>:<REMOTE_DIR> (eg. --map-aliases /Users/<User Name>:/Volumes/Home).  \
                Multiple maps may be specified delimited by a comma, ','.  You may also set via the environment variable HTTM_MAP_ALIASES.")
                .use_value_delimiter(true)
                .takes_value(true)
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(21)
        )
        .arg(
            Arg::new("NUM_VERSIONS")
                .long("num-versions")
                .default_missing_value("all")
                .possible_values(&["all", "single", "single-no-snap", "single-with-snap", "multiple"])
                .min_values(0)
                .require_equals(true)
                .help("detect and display the number of versions available (e.g. one, \"1\", \
                version is available if either a snapshot version exists, and is identical to live version, or only a live version exists).  \
                This option takes a value: \"all\" will print the filename and number of versions, \
                \"single\" will print only filenames which only have one version, \
                (and \"single-no-snap\" will print those without a snap taken, and \"single-with-snap\" will print those with a snap taken), \
                and \"multiple\" will print only filenames which only have multiple versions.")
                .conflicts_with_all(&["LAST_SNAP", "INTERACTIVE", "SELECT", "RESTORE", "RECURSIVE", "SNAP_FILE_MOUNT", "LAST_SNAP", "NOT_SO_PRETTY", "NO_LIVE", "NO_SNAP", "OMIT_IDENTICAL"])
                .display_order(22)
        )
        .arg(
            Arg::new("REMOTE_DIR")
                .long("remote-dir")
                .hide(true)
                .visible_aliases(&["remote", "snap-point"])
                .help("DEPRECATED.  Use MAP_ALIASES. Manually specify that mount point for ZFS (directory which contains a \".zfs\" directory) or btrfs-snapper \
                (directory which contains a \".snapshots\" directory), such as the local mount point for a remote share.  You may also set via the HTTM_REMOTE_DIR environment variable.")
                .takes_value(true)
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(23)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .hide(true)
                .visible_alias("local")
                .help("DEPRECATED.  Use MAP_ALIASES.  Used with \"remote-dir\" to determine where the corresponding live root filesystem of the dataset is.  \
                Put more simply, the \"local-dir\" is likely the directory you backup to your \"remote-dir\".  If not set, httm defaults to your current working directory.  \
                You may also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("REMOTE_DIR")
                .takes_value(true)
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(24)
        )
        .arg(
            Arg::new("UTC")
                .long("utc")
                .help("use UTC for date display and timestamps")
                .display_order(25)
        )
        .arg(
            Arg::new("DEBUG")
                .long("debug")
                .help("print configuration and debugging info")
                .display_order(26)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(27)
        )
        .get_matches()
}

#[derive(Debug, Clone)]
pub struct Config {
    pub paths: Vec<PathData>,
    pub opt_raw: bool,
    pub opt_zeros: bool,
    pub opt_no_pretty: bool,
    pub opt_no_live: bool,
    pub opt_recursive: bool,
    pub opt_exact: bool,
    pub opt_overwrite: bool,
    pub opt_no_filter: bool,
    pub opt_no_snap: bool,
    pub opt_debug: bool,
    pub opt_no_traverse: bool,
    pub opt_omit_ditto: bool,
    pub opt_last_snap: Option<LastSnapMode>,
    pub opt_preview: Option<String>,
    pub requested_utc_offset: UtcOffset,
    pub exec_mode: ExecMode,
    pub dataset_collection: FilesystemInfo,
    pub deleted_mode: Option<DeletedMode>,
    pub pwd: PathData,
    pub opt_requested_dir: Option<PathData>,
}

impl Config {
    pub fn new() -> HttmResult<Self> {
        let arg_matches = parse_args();
        Config::from_matches(arg_matches)
    }

    fn from_matches(matches: ArgMatches) -> HttmResult<Self> {
        if matches.is_present("ZSH_HOT_KEYS") {
            install_hot_keys()?
        }

        let requested_utc_offset = if matches.is_present("UTC") {
            UtcOffset::UTC
        } else {
            // this fn is surprisingly finicky. it needs to be done
            // when program is not multithreaded, etc., so we don't even print an
            // error and we just default to UTC if something fails
            UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
        };

        let opt_zeros = matches.is_present("ZEROS");
        let opt_no_snap = matches.is_present("NO_SNAP");
        // force a raw mode if one is not set for no_snap mode
        let mut opt_raw = matches.is_present("RAW") || opt_no_snap && !opt_zeros;
        let opt_no_pretty = matches.is_present("NOT_SO_PRETTY");
        let opt_recursive = matches.is_present("RECURSIVE");
        let opt_exact = matches.is_present("EXACT");
        let opt_no_live = matches.is_present("NO_LIVE");
        let opt_no_filter = matches.is_present("NO_FILTER");
        let opt_debug = matches.is_present("DEBUG");
        let opt_overwrite = matches!(
            matches.value_of("RESTORE"),
            Some("overwrite") | Some("yolo")
        );

        let opt_last_snap = match matches.value_of("LAST_SNAP") {
            Some("") | Some("any") => Some(LastSnapMode::Any),
            Some("none") => Some(LastSnapMode::None),
            Some("ditto") => Some(LastSnapMode::DittoOnly),
            Some("no-ditto-inclusive") => Some(LastSnapMode::NoDittoInclusive),
            Some("no-ditto-exclusive") | Some("no-ditto") => Some(LastSnapMode::NoDittoExclusive),
            _ => None,
        };

        let opt_num_versions = match matches.value_of("NUM_VERSIONS") {
            Some("") | Some("all") => Some(NumVersionsMode::All),
            Some("single") => Some(NumVersionsMode::SingleAll),
            Some("single-no-snap") => Some(NumVersionsMode::SingleNoSnap),
            Some("single-with-snap") => Some(NumVersionsMode::SingleWithSnap),
            Some("multiple") => Some(NumVersionsMode::Multiple),
            _ => None,
        };

        let opt_preview = match matches.value_of("PREVIEW") {
            Some("") | Some("default") => Some("default".to_owned()),
            _ => None,
        };

        let mut deleted_mode = match matches.value_of("DELETED_MODE") {
            Some("") | Some("all") => Some(DeletedMode::Enabled),
            Some("single") => Some(DeletedMode::DepthOfOne),
            Some("only") => Some(DeletedMode::Only),
            _ => None,
        };

        let opt_interactive_mode = if matches.is_present("RESTORE") {
            Some(InteractiveMode::Restore)
        } else if matches.is_present("SELECT") {
            Some(InteractiveMode::Select)
        } else if matches.is_present("BROWSE") {
            Some(InteractiveMode::Browse)
        } else {
            None
        };

        // if in last snap and select mode we will want to return a raw value,
        // better to have this here.  It's more confusing if we work this logic later, I think.
        if opt_last_snap.is_some() && matches!(opt_interactive_mode, Some(InteractiveMode::Select))
        {
            opt_raw = true
        }

        let opt_snap_file_mount =
            if let Some(requested_snapshot_suffix) = matches.value_of("SNAP_FILE_MOUNT") {
                if requested_snapshot_suffix == "httmSnapFileMount" {
                    Some(requested_snapshot_suffix.to_owned())
                } else if requested_snapshot_suffix.contains(char::is_whitespace) {
                    return Err(HttmError::new(
                        "httm will only accept snapshot suffixes which don't contain whitespace",
                    )
                    .into());
                } else {
                    Some(requested_snapshot_suffix.to_owned())
                }
            } else {
                None
            };

        let mut exec_mode = if let Some(num_versions_mode) = opt_num_versions {
            ExecMode::NumVersions(num_versions_mode)
        } else if matches.is_present("MOUNT_FOR_FILE") {
            ExecMode::MountsForFiles
        } else if let Some(requested_snapshot_suffix) = opt_snap_file_mount {
            ExecMode::SnapFileMount(requested_snapshot_suffix)
        } else if let Some(interactive_mode) = opt_interactive_mode {
            ExecMode::Interactive(interactive_mode)
        } else if deleted_mode.is_some() && opt_recursive {
            let progress_bar: ProgressBar = indicatif::ProgressBar::new_spinner();
            ExecMode::DisplayRecursive(progress_bar)
        } else {
            // no need for deleted file modes in a non-interactive/display recursive setting
            deleted_mode = None;
            ExecMode::Display
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
        let pwd = Self::get_pwd()?;

        // paths are immediately converted to our PathData struct
        let paths: Vec<PathData> =
            Self::get_paths(matches.values_of_os("INPUT_FILES"), &exec_mode, &pwd)?;

        // for exec_modes in which we can only take a single directory, process how we handle those here
        let opt_requested_dir: Option<PathData> =
            Self::get_opt_requested_dir(&mut exec_mode, &mut deleted_mode, &paths, &pwd)?;

        let opt_omit_ditto = matches.is_present("OMIT_DITTO");

        // opt_omit_identical doesn't make sense in Display Recursive mode as no live files will exists?
        if opt_omit_ditto && matches!(exec_mode, ExecMode::DisplayRecursive(_)) {
            return Err(HttmError::new("Omit identical mode not available when a deleted recursive search is specified.  Quitting.").into());
        }

        if opt_last_snap.is_some() && matches!(exec_mode, ExecMode::DisplayRecursive(_)) {
            return Err(
                HttmError::new("Last snap is not available in Display Recursive Mode.").into(),
            );
        }

        // doesn't make sense to follow symlinks when you're searching the whole system,
        // so we disable our bespoke "when to traverse symlinks" algo here, or if requested.
        let opt_no_traverse = matches.is_present("NO_TRAVERSE") || {
            if let Some(user_requested_dir) = opt_requested_dir.as_ref() {
                user_requested_dir.path_buf == Path::new(ROOT_DIRECTORY)
            } else {
                false
            }
        };

        // obtain a map of datasets, a map of snapshot directories, and possibly a map of
        // alternate filesystems and map of aliases if the user requests
        let dataset_collection = FilesystemInfo::new(
            matches.is_present("ALT_REPLICATED"),
            matches.value_of_os("REMOTE_DIR"),
            matches.value_of_os("LOCAL_DIR"),
            matches.values_of_os("MAP_ALIASES"),
            &pwd,
            &exec_mode,
        )?;

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
            opt_no_traverse,
            opt_omit_ditto,
            opt_last_snap,
            opt_preview,
            requested_utc_offset,
            dataset_collection,
            exec_mode,
            deleted_mode,
            pwd,
            opt_requested_dir,
        };

        Ok(config)
    }

    pub fn get_pwd() -> HttmResult<PathData> {
        if let Ok(pwd) = std::env::current_dir() {
            if let Ok(path) = PathBuf::from(&pwd).canonicalize() {
                Ok(PathData::from(path.as_path()))
            } else {
                Err(
                    HttmError::new("Could not obtain a canonical path for your working directory")
                        .into(),
                )
            }
        } else {
            Err(HttmError::new(
                "Working directory does not exist or your do not have permissions to access it.",
            )
            .into())
        }
    }

    pub fn get_paths(
        opt_os_values: Option<OsValues>,
        exec_mode: &ExecMode,
        pwd: &PathData,
    ) -> HttmResult<Vec<PathData>> {
        let mut paths = if let Some(input_files) = opt_os_values {
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
                ExecMode::Interactive(_) | ExecMode::DisplayRecursive(_) => {
                    vec![pwd.clone()]
                }
                ExecMode::Display
                | ExecMode::SnapFileMount(_)
                | ExecMode::MountsForFiles
                | ExecMode::NumVersions(_) => read_stdin()?
                    .par_iter()
                    .map(|string| PathData::from(Path::new(&string)))
                    .collect(),
            }
        };

        // deduplicate pathdata and sort if in display mode --
        // so input of ./.z* and ./.zshrc will only print ./.zshrc once
        paths = if paths.len() > 1 {
            paths.sort_unstable();
            // dedup needs to be sorted/ordered first to work (not like a BTreeMap)
            paths.dedup();

            paths
        } else {
            paths
        };

        Ok(paths)
    }

    pub fn get_opt_requested_dir(
        exec_mode: &mut ExecMode,
        deleted_mode: &mut Option<DeletedMode>,
        paths: &[PathData],
        pwd: &PathData,
    ) -> HttmResult<Option<PathData>> {
        let res = match exec_mode {
            ExecMode::Interactive(_) | ExecMode::DisplayRecursive(_) => {
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
                                ExecMode::Interactive(ref interactive_mode) => {
                                    match interactive_mode {
                                        InteractiveMode::Browse => {
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
                                ExecMode::DisplayRecursive(_) => {
                                    *exec_mode = ExecMode::Display;
                                    *deleted_mode = None;
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
            ExecMode::Display
            | ExecMode::SnapFileMount(_)
            | ExecMode::MountsForFiles
            | ExecMode::NumVersions(_) => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                None
            }
        };
        Ok(res)
    }
}
