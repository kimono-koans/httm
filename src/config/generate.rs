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
// Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use crate::config::install_hot_keys::install_hot_keys;
use crate::data::paths::{PathData, PathDeconstruction, ZfsSnapPathGuard};
use crate::filesystem::collection::FilesystemInfo;
use crate::filesystem::mounts::{FilesystemType, ROOT_PATH};
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::pwd;
use crate::lookup::file_mounts::MountDisplay;
use clap::parser::ValuesRef;
use clap::{Arg, ArgAction, ArgMatches, crate_name, crate_version};
use indicatif::ProgressBar;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::borrow::Cow;
use std::fs::read_link;
use std::io::Read;
use std::ops::Index;
use std::path::{Path, PathBuf};
use time::UtcOffset;

#[derive(Debug, Clone)]
pub enum ExecMode {
    Interactive(InteractiveMode),
    NonInteractiveRecursive(indicatif::ProgressBar),
    BasicDisplay,
    Preview,
    SnapFileMount(String),
    Prune(Option<ListSnapsFilters>),
    MountsForFiles(MountDisplay),
    SnapsForFiles(Option<ListSnapsFilters>),
    NumVersions(NumVersionsMode),
    RollForward(String),
}

#[derive(Debug, Clone)]
pub enum BulkExclusion {
    NoLive,
    NoSnap,
}

#[derive(Debug, Clone)]
pub enum InteractiveMode {
    Browse,
    Select(SelectMode),
    Restore(RestoreMode),
}

#[derive(Debug, Clone)]
pub enum RestoreSnapGuard {
    Guarded,
    NotGuarded,
}

#[derive(Debug, Clone)]
pub enum SelectMode {
    Path,
    Contents,
    Preview,
}

#[derive(Debug, Clone)]
pub enum RestoreMode {
    CopyOnly,
    CopyAndPreserve,
    Overwrite(RestoreSnapGuard),
}

#[derive(Debug, Clone)]
pub enum PrintMode {
    Formatted(FormattedMode),
    Raw(RawMode),
}

#[derive(Debug, Clone)]
pub enum RawMode {
    Csv,
    Newline,
    Zero,
}

#[derive(Debug, Clone)]
pub enum FormattedMode {
    Default,
    NotPretty,
}

#[derive(Debug, Clone)]
pub enum DeletedMode {
    DepthOfOne,
    All,
    Only,
}

#[derive(Debug, Clone)]
pub enum DedupBy {
    Disable,
    Metadata,
    Contents,
    Suspect,
}

#[derive(Debug, Clone)]
pub struct ListSnapsFilters {
    pub select_mode: bool,
    pub omit_num_snaps: usize,
    pub name_filters: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum LastSnapMode {
    Any,
    Without,
    DittoOnly,
    NoDittoExclusive,
    NoDittoInclusive,
}

#[derive(Debug, Clone)]
pub enum NumVersionsMode {
    AllNumerals,
    AllGraph,
    SingleAll,
    SingleNoSnap,
    SingleWithSnap,
    Multiple,
}

const NATIVE_SNAP_SUFFIXES: [&str; 4] = [
    "ounceSnapFileMount",
    "httmSnapFileMount",
    "httmSnapRollForward",
    "httmSnapRestore",
];

#[inline(always)]
fn parse_args() -> ArgMatches {
    clap::command!(crate_name!())
        .about("httm prints the size, date and corresponding locations of available unique versions of files residing on snapshots. \
        May also be used interactively to select and restore from such versions, and even to snapshot datasets which contain certain files.")
        .version(crate_version!())
        .arg(
            Arg::new("INPUT_FILES")
                .help("in the non-interactive modes (when BROWSE, SELECT, or RESTORE are not specified), \
                user may specify one or many paths for a simple display of snapshot versions.  \
                If no paths are included as arguments, then httm will pause waiting for paths to be piped in via stdin (e.g. 'find . | httm').  \
                In the interactive modes (when BROWSE, SELECT, or RESTORE are specified), user may specify one base directory from which to begin a search.  \
                If the interactive modes, if no directory is specified, httm will use the current working directory.")
                .value_parser(clap::value_parser!(PathBuf))
                .num_args(0..)
                .display_order(1)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("BROWSE")
                .short('b')
                .short_alias('i')
                .long("browse")
                .visible_alias("interactive")
                .help("interactive browse and search a specified directory to display unique file versions.")
                .display_order(2)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("SELECT")
                .short('s')
                .long("select")
                .value_parser(["path", "contents", "preview"])
                .num_args(0..=1)
                .default_missing_value("path")
                .require_equals(true)
                .help("interactive browse and search a specified directory to display unique file versions. \
                Continue to another dialog to select a snapshot version to dump to stdout. This argument optionally takes a value. \
                Default behavior/value is to simply print the path name, but, if the path is a file, the user can print the file's contents by giving the value \"contents\", \
                or print any PREVIEW output by giving the value \"preview\".")
                .conflicts_with("RESTORE")
                .display_order(3)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("COPY")
                .short('c')
                .long("copy")
                .value_parser(["copy", "copy-and-preserve", "preserve"])
                .num_args(0..=1)
                .default_missing_value("copy")
                .require_equals(true)
                .help("interactive browse and search a specified directory to display unique file versions. Continue to another dialog to select a snapshot version to copy. \
                This argument optionally takes a value. Default behavior/value is a non-destructive \"copy\" to the current working directory with a new name, \
                so as not to overwrite any \"live\" file version. However, the user may specify the \"preserve\" value.  \
                Preserve mode will attempt to preserve attributes, like the permissions/mode, timestamps, xattrs and ownership of the selected snapshot file version.  \
                User may also set the copy/restore mode via the HTTM_RESTORE_MODE environment variable.")
                .conflicts_with_all(&["BROWSE", "SELECT", "RESTORE"])
                .display_order(4)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("RESTORE")
                .short('r')
                .long("restore")
                .value_parser(["overwrite", "yolo", "guard"])
                .num_args(0..=1)
                .default_missing_value("overwrite")
                .require_equals(true)
                .help("interactive browse and search a specified directory to display unique file versions. Continue to another dialog to select a snapshot version to restore. \
                This argument optionally takes a value. Default behavior/value is \"overwrite\" (or \"yolo\") to restore to the same file location. Note, \"overwrite\" can be a DESTRUCTIVE operation. \
                Overwrite mode will attempt to preserve attributes, like the permissions/mode, timestamps, xattrs and ownership of the selected snapshot file version. \
                User may also specify \"guard\".  Guard mode has the same semantics as \"overwrite\" but will attempt to take a precautionary snapshot before any overwrite action occurs. \
                Note: Guard mode is a ZFS only option. User may also set the copy/restore mode via the HTTM_RESTORE_MODE environment variable.")
                .conflicts_with_all(&["BROWSE", "SELECT", "COPY"])
                .display_order(5)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("DELETED")
                .short('d')
                .long("deleted")
                .default_missing_value("all")
                .value_parser(["all", "single", "only", "one"])
                .num_args(0..=1)
                .require_equals(true)
                .help("show deleted files in interactive modes (BROWSE, SELECT and RESTORE). When an interactive mode is not specified, search for all files deleted from a specified directory. \
                This argument optionally takes a value. When specified, the default behavior/value is \"all\". \
                If \"only\" is specified, then, in the interactive modes, non-deleted files will be excluded from the search. \
                If \"single\" is specified, then, deleted files behind deleted directories, (files with a depth greater than one) will be ignored.")
                .display_order(6)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("RECURSIVE")
                .short('R')
                .long("recursive")
                .conflicts_with_all(&["SNAPSHOT"])
                .help("recurse into the selected directory to find more files and directories. Only available in interactive (BROWSE, SELECT and RESTORE) and DELETED file modes.")
                .display_order(7)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("ALT_REPLICATED")
                .short('a')
                .long("alt-replicated")
                .aliases(["replicated"])
                .help("automatically discover locally replicated datasets and list their snapshots as well. \
                NOTE: Be certain such replicated datasets are mounted before use. \
                httm will silently ignore unmounted datasets in the interactive modes.")
                .conflicts_with_all(&["REMOTE_DIR", "LOCAL_DIR"])
                .display_order(8)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("PREVIEW")
                .short('p')
                .long("preview")
                .help("user may specify a command to preview snapshots while in a snapshot selection view.  \
                This argument optionally takes a value specifying the command to be executed.  \
                The default value/command, if no command value specified, is a 'bowie' formatted 'diff'.  \
                User defined commands must specify the snapshot file name \"{snap_file}\" and the live file name \"{live_file}\" within their shell command. \
                NOTE: 'bash' is required to bootstrap any preview script, even if the user specifies their own preview command, written in a different shell language.")
                .value_parser(clap::value_parser!(String))
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("default")
                .display_order(9)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("DEDUP_BY")
                .long("dedup-by")
                .value_parser(["disable", "all", "no-filter", "metadata", "contents", "suspect"])
                .num_args(0..=1)
                .visible_aliases(&["unique", "uniqueness"])
                .default_missing_value("contents")
                .require_equals(true)
                .help("comparing file versions solely on the basis of size and modify time (the default \"metadata\" behavior) may return what appear to be \"false positives\".  \
                This is because metadata, specifically modify time and size, is not a precise measure of whether a file has actually changed. A program might overwrite a file with the same contents, \
                and/or a user can simply update the modify time via 'touch'. This flag, when specified with the \"contents\" option, compares the actual file contents of same-sized file versions, \
                overriding the default \"metadata\" only behavior. The \"contents\" option can be expensive, as the file versions need to be read back and compared, and, thus, should probably only be used for smaller files. \
                Given how expensive this operation can be, for larger files or files with many versions, if specified, the \"contents\" option is not shown in BROWSE mode, \
                but after a selection is made, can be utilized, when enabled, in SELECT or RESTORE modes.  A less expensive, \"suspect\" option, only compares file contents when a file's metadata makes it likely the file may have new file contents, \
                such as when that file has a new inode or birth time.  The \"disable\" \"all\" or \"no-filter\" option dumps all snapshot versions, without deduplication.")
                .display_order(10)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("EXACT")
                .short('e')
                .long("exact")
                .help("use exact pattern matching for searches in the interactive modes (in contrast to the default fuzzy searching).")
                .display_order(11)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("SNAPSHOT")
                .short('S')
                .long("snap")
                .require_equals(true)
                .default_missing_value("httmSnapFileMount")
                .num_args(0..=1)
                .value_parser(clap::value_parser!(String))
                .help("snapshot a file/s most immediate mount. \
                This argument optionally takes a value for a snapshot suffix. The default suffix is 'httmSnapFileMount'. \
                Note: This is a ZFS only option which requires either superuser or 'zfs allow' privileges.")
                .conflicts_with_all(&["BROWSE", "SELECT", "RESTORE", "ALT_REPLICATED", "REMOTE_DIR", "LOCAL_DIR"])
                .display_order(12)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("LIST_SNAPS")
                .long("list-snaps")
                .aliases(&["snap-names", "snaps-for-file", "ls-snaps", "list-snapshots"])
                .value_parser(clap::value_parser!(String))
                .num_args(0..=1)
                .require_equals(true)
                .help("display snapshots names for a file. This argument optionally takes a value. \
                By default, this argument will return all available snapshot names. \
                When the DEDUP_BY flag is not specified, but LIST_SNAPS is, the default DEDUP_BY level is \"all\" snapshots. \
                Thus the user may limit type of snapshots returned via specifying the DEDUP_BY flag. \
                The user may also omit the most recent \"n\" snapshots from any list. \
                By appending a comma, this argument also filters those snapshots which contain the specified pattern/s. \
                A value of \"5,prep_Apt\" would return the snapshot names of only the last 5 (at most) of all snapshot versions which contain \"prep_Apt\". \
                The value \"native\" will restrict selection to only 'httm' native snapshot suffix values, like \"httmSnapFileMount\" and \"ounceSnapFileMount\". \
                Note: This is a ZFS and btrfs only option.")
                .conflicts_with_all(&["BROWSE", "RESTORE"])
                .display_order(13)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("ROLL_FORWARD")
                .long("roll-forward")
                .aliases(&["roll", "spring", "spring-forward"])
                .value_parser(clap::value_parser!(String))
                .num_args(1)
                .require_equals(true)
                .help("whereas 'zfs rollback' is a destructive operation, this operation is non-destructive. \
                This operation preserves interstitial snapshots, and requires a snapshot guard before taking any action.  \
                If this flag is specified (along with the required snapshot name), \
                httm will modify (copy and delete) those files and their attributes (preserving hard links) that have changed since the specified snapshot to the live dataset. \
                httm will also take two precautionary guard snapshots, one before and one after the operation. \
                Should the roll forward fail for any reason, httm will rollback to the pre-execution state. \
                CAVEATS: This is a ZFS only option which requires super user privileges.  \
                Not all filesystem features are supported (for instance, Unix sockets on the snapshot will need to be recreated) and may cause a roll forward to fail.  \
                Moreover, certain special objects/files will be copied or recreated, but are not guaranteed to be in the same state as the snapshot (for instance, FIFO buffers).")
                .conflicts_with_all(&["BROWSE", "RESTORE", "ALT_REPLICATED", "REMOTE_DIR", "LOCAL_DIR"])
                .display_order(14)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("PRUNE")
                .long("prune")
                .aliases(&["purge"])
                .help("prune all snapshot/s which contain the input file/s on that file's most immediate mount via \"zfs destroy\". \
                \"zfs destroy\" is a DESTRUCTIVE operation which DOES NOT ONLY APPLY to the file in question, but the entire snapshot upon which it resides. \
                Careless use may cause you to lose snapshot data you care about. \
                This argument requires and will be filtered according to any values specified at LIST_SNAPS. \
                User may also enable SELECT mode to make a more granular selection of specific snapshots to prune. \
                Note: This is a ZFS only option.")
                .conflicts_with_all(&["BROWSE", "RESTORE", "ALT_REPLICATED", "REMOTE_DIR", "LOCAL_DIR"])                
                .display_order(15)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("FILE_MOUNT")
                .short('m')
                .long("file-mount")
                .alias("mount-for-file")
                .visible_alias("mount")
                .default_missing_value("target")
                .value_parser(["source", "target", "mount", "directory", "device", "dataset", "relative-path", "relative", "relpath"])
                .num_args(0..=1)
                .require_equals(true)
                .help("by default, display the all mount point/s of all dataset/s which contain/s the input file/s. \
                This argument optionally takes a value to display other information about the path. Possible values are: \
                \"mount\" or \"target\" or \"directory\", the default value, returns the mount/directory of a file's underlying dataset, \
                \"source\" or \"device\" or \"dataset\", returns a file's underlying dataset/device, and, \
                \"relative-path\" or \"relative\", returns a file's relative path from the underlying mount.")
                .conflicts_with_all(&["BROWSE", "SELECT", "RESTORE"])
                .display_order(16)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("LAST_SNAP")
                .short('l')
                .long("last-snap")
                .default_missing_value("any")
                .visible_aliases(&["last", "latest"])
                .value_parser(["any", "ditto", "no-ditto", "no-ditto-exclusive", "no-ditto-inclusive", "none", "without"])
                .num_args(0..=1)
                .require_equals(true)
                .help("automatically select and print the path of last-in-time unique snapshot version for the input file. \
                This argument optionally takes a value. Possible values are: \
                \"any\", return the last in time snapshot version, this is the default behavior/value, \
                \"ditto\", return only last snaps which are the same as the live file version, \
                \"no-ditto-exclusive\", return only a last snap which is not the same as the live version (argument \"--no-ditto\" is an alias for this option), \
                \"no-ditto-inclusive\", return a last snap which is not the same as the live version, or should none exist, return the live file, and, \
                \"none\" or \"without\", return the live file only for those files without a last snapshot.")
                .conflicts_with_all(&["NUM_VERSIONS", "SNAPSHOT", "FILE_MOUNT", "ALT_REPLICATED", "REMOTE_DIR", "LOCAL_DIR", "PREVIEW"])
                .display_order(17)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("RAW")
                .short('n')
                .long("raw")
                .visible_alias("newline")
                .help("display the snapshot locations only, without extraneous information, delimited by a NEWLINE character.")
                .conflicts_with_all(&["ZEROS", "CSV", "NOT_SO_PRETTY"])
                .display_order(18)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("ZEROS")
                .short('0')
                .long("zero")
                .visible_alias("null")
                .help("display the snapshot locations only, without extraneous information, delimited by a NULL character.")
                .conflicts_with_all(&["RAW", "CSV", "NOT_SO_PRETTY"])
                .display_order(19)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("CSV")
                .long("csv")
                .help("display all information, delimited by a comma.")
                .conflicts_with_all(&["RAW", "ZEROS", "NOT_SO_PRETTY", "JSON"])
                .display_order(20)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NOT_SO_PRETTY")
                .long("not-so-pretty")
                .visible_aliases(&["tabs", "plain-jane", "not-pretty"])
                .help("display the ordinary output, but tab delimited, without any pretty border lines.")
                .conflicts_with_all(&["RAW", "ZEROS", "CSV"])
                .display_order(21)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("JSON")
                .long("json")
                .help("display the ordinary output, but as formatted JSON.")
                .conflicts_with_all(&["SELECT", "RESTORE"])
                .display_order(22)
                .conflicts_with_all(&["CSV"])
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("OMIT_DITTO")
                .long("omit-ditto")
                .help("omit display of the snapshot version which may be identical to any live version. By default, `httm` displays all snapshot versions and the live version).")
                .conflicts_with_all(&["NUM_VERSIONS"])
                .display_order(23)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NO_FILTER")
                .long("no-filter")
                .help("by default, in the interactive modes, httm will filter out files residing upon non-supported datasets (like ext4, tmpfs, procfs, sysfs, or devtmpfs, etc.), and within any \"common\" snapshot paths. \
                Here, one may select to disable such filtering. Note, httm will always show the input path, and results from behind any input path when that is the directory path being searched.") 
                .display_order(24)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("FILTER_HIDDEN")
                .long("no-hidden")
                .aliases(&["no-hide", "nohide", "filter-hidden"])
                .help("do not show information regarding hidden files and directories (those that start with a \'.\') in the recursive or interactive modes.")
                .display_order(25)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("ONE_FILESYSTEM")
                .long("one-filesystem")
                .aliases(&["same-filesystem", "single-filesystem", "one-fs", "onefs"])
                .requires("RECURSIVE")
                .help("limit recursive search to file and directories on the same filesystem/device as the target directory.")
                .display_order(26)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NO_TRAVERSE")
                .long("no-traverse")
                .help("in recursive mode, don't traverse symlinks. Although httm does its best to prevent searching pathologically recursive symlink-ed paths, \
                here, you may disable symlink traversal completely. NOTE: httm will never traverse symlinks when a requested recursive search is on the root/base directory (\"/\").")
                .display_order(27)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NO_LIVE")
                .long("no-live")
                .visible_aliases(&["dead", "disco"])
                .help("only display information concerning snapshot versions (display no information regarding live versions of files or directories) in any Display Recursive mode (when DELETED and RECURSIVE are specified, but not an interactive mode).")
                .conflicts_with_all(&["BROWSE", "SELECT", "RESTORE", "SNAPSHOT", "LAST_SNAP", "NOT_SO_PRETTY"])
                .display_order(28)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NO_SNAP")
                .long("no-snap")
                .visible_aliases(&["undead", "zombie"])
                .help("only display information concerning 'pseudo-live' versions in any Display Recursive mode (when DELETED and RECURSIVE are specified, but not an interactive mode). \
                Useful for finding the \"files that once were\" and displaying only those pseudo-live/zombie files.")
                .conflicts_with_all(&["BROWSE", "SELECT", "RESTORE", "SNAPSHOT", "LAST_SNAP", "NOT_SO_PRETTY"])
                .display_order(29)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("ALT_STORE")
                .long("alt-store")
                .alias("store")
                .require_equals(true)
                .value_parser(["restic", "timemachine"])
                .help("give priority to specified alternative backups stores, like Restic, and Time Machine.  \
                If this flag is specified, httm will place any discovered alternative backups store as priority snapshots for the root mount point (\"/\"), \
                ignoring other, potentially more direct, mounts.  Before use, be sure that any such repository is mounted.  \
                You may need superuser privileges to view a repository mounted with superuser permission.  \
                NOTE: httm includes a helper script called \"equine\" which can assist you in mounting remote and local Time Machine snapshots.")
                .conflicts_with_all(["MAP_ALIASES"])
                .display_order(30)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("MAP_ALIASES")
                .long("map-aliases")
                .visible_aliases(&["aliases"])
                .help("manually map a local directory (eg. \"/Users/<User Name>\") as an alias of a mount point for ZFS or btrfs, \
                such as the local mount point for a backup on a remote share (eg. \"/Volumes/Home\"). \
                This option is useful if you wish to view snapshot versions from within the local directory you back up to a remote network share. \
                This option requires a value pair. Each pair is delimited by a colon, ':', and is specified in the form <LOCAL_DIR>:<REMOTE_DIR> \
                (eg. --map-aliases /Users/<User Name>:/Volumes/Home). Multiple maps may be specified delimited by a comma, ','. \
                You may also set via the environment variable HTTM_MAP_ALIASES.")
                .use_value_delimiter(true)
                .value_parser(clap::builder::ValueParser::os_string())
                .num_args(0..=1)
                .display_order(31)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("NUM_VERSIONS")
                .long("num-versions")
                .default_missing_value("all")
                .value_parser(["all", "graph", "single", "single-no-snap", "single-with-snap", "multiple"])
                .num_args(0..=1)
                .require_equals(true)
                .help("detect and display the number of unique versions available (e.g. one, \"1\", \
                version is available if either a snapshot version exists, and is identical to live version, or only a live version exists). \
                This argument optionally takes a value. The default value, \"all\", will print the filename and number of versions, \
                \"graph\" will print the filename and a line of characters representing the number of versions, \
                \"single\" will print only filenames which only have one version, \
                (and \"single-no-snap\" will print those without a snap taken, and \"single-with-snap\" will print those with a snap taken), \
                and \"multiple\" will print only filenames which only have multiple versions.")
                .conflicts_with_all(&["LAST_SNAP", "BROWSE", "SELECT", "RESTORE", "RECURSIVE", "SNAPSHOT", "NO_LIVE", "NO_SNAP", "OMIT_DITTO"])
                .display_order(32)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("REMOTE_DIR")
                .long("remote-dir")
                .hide(true)
                .visible_aliases(&["remote", "snap-point"])
                .help("DEPRECATED. Use MAP_ALIASES. Manually specify that mount point for ZFS (directory which contains a \".zfs\" directory) or btrfs-snapper \
                (directory which contains a \".snapshots\" directory), such as the local mount point for a remote share. You may also set via the HTTM_REMOTE_DIR environment variable.")
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(33)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("LOCAL_DIR")
                .long("local-dir")
                .hide(true)
                .visible_alias("local")
                .help("DEPRECATED. Use MAP_ALIASES. Used with \"remote-dir\" to determine where the corresponding live root filesystem of the dataset is. \
                Put more simply, the \"local-dir\" is likely the directory you backup to your \"remote-dir\". If not set, httm defaults to your current working directory. \
                You may also set via the environment variable HTTM_LOCAL_DIR.")
                .requires("REMOTE_DIR")
                .value_parser(clap::builder::ValueParser::os_string())
                .display_order(34)
                .action(ArgAction::Append)
        )
        .arg(
            Arg::new("UTC")
                .long("utc")
                .help("use UTC for date display and timestamps")
                .display_order(35)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("NO_CLONES")
                .long("no-clones")
                .help("by default, when copying files from snapshots, httm will first attempt a zero copy \"reflink\" clone on systems that support it. \
                Here, you may disable that behavior, and force httm to use the default copy behavior. \
                You may also set an environment variable to any value, \"HTTM_NO_CLONE\" to disable.")
                .display_order(36)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("LAZY_SNAP_EVAL")
                .long("lazy")
                .short('L')
                .aliases(&["realtime", "lazy-snap"])
                .help("by default, all snapshot locations are discovered at initial program execution, however, here, \
                a user may request that the program lazily wait until a search is executed before resolving any path's snapshot locations.  \
                This provides the most accurate snapshot versions possible, but, given the additional metadata IO, may feel slower on older systems, with only marginal benefit.  \
                NOTE: This option is also only available on filesystems with well defined snapshot locations (that is, not BTRFS datasets).")
                .display_order(37)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("DEBUG")
                .long("debug")
                .help("print configuration and debugging info")
                .display_order(38)
                .action(ArgAction::SetTrue)
        )
        .arg(
            Arg::new("ZSH_HOT_KEYS")
                .long("install-zsh-hot-keys")
                .help("install zsh hot keys to the users home directory, and then exit")
                .exclusive(true)
                .display_order(39)
                .action(ArgAction::SetTrue)
        )
        .get_matches()
}

#[derive(Debug, Clone)]
pub struct Config {
    pub paths: Vec<PathData>,
    pub opt_recursive: bool,
    pub opt_exact: bool,
    pub opt_no_filter: bool,
    pub opt_debug: bool,
    pub opt_no_traverse: bool,
    pub opt_omit_ditto: bool,
    pub opt_no_hidden: bool,
    pub opt_json: bool,
    pub opt_one_filesystem: bool,
    pub opt_no_clones: bool,
    pub opt_lazy: bool,
    pub dedup_by: DedupBy,
    pub opt_bulk_exclusion: Option<BulkExclusion>,
    pub opt_last_snap: Option<LastSnapMode>,
    pub opt_preview: Option<String>,
    pub opt_deleted_mode: Option<DeletedMode>,
    pub opt_requested_dir: Option<PathBuf>,
    pub requested_utc_offset: UtcOffset,
    pub exec_mode: ExecMode,
    pub print_mode: PrintMode,
    pub dataset_collection: FilesystemInfo,
    pub pwd: PathBuf,
}

impl TryFrom<&ArgMatches> for Config {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(matches: &ArgMatches) -> HttmResult<Self> {
        if matches.get_flag("ZSH_HOT_KEYS") {
            install_hot_keys()?
        }

        let requested_utc_offset = if matches.get_flag("UTC") {
            UtcOffset::UTC
        } else {
            // this fn is surprisingly finicky. it needs to be done
            // when program is not multithreaded, etc., so we don't even print an
            // error and we just default to UTC if something fails
            UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC)
        };

        let opt_debug = matches.get_flag("DEBUG");

        // current working directory will be helpful in a number of places
        let pwd = pwd()?;

        // obtain a map of datasets, a map of snapshot directories, and possibly a map of
        // alternate filesystems and map of aliases if the user requests
        let mut opt_map_aliases: Option<Vec<Cow<str>>> = matches
            .get_raw("MAP_ALIASES")
            .map(|aliases| aliases.map(|os_str| os_str.to_string_lossy()).collect());

        let opt_alt_store: Option<FilesystemType> = match matches
            .get_one::<String>("ALT_STORE")
            .map(|inner| inner.as_str())
        {
            Some("timemachine") => Some(FilesystemType::Apfs),
            Some("restic") => Some(FilesystemType::Restic(None)),
            _ => None,
        };

        if opt_alt_store.is_some() && opt_map_aliases.is_some() {
            eprintln!(
                "WARN: httm has disabled any MAP_ALIASES in preference to an ALT_STORE specified."
            );
            opt_map_aliases = None;
        }

        let opt_alt_replicated = matches.get_flag("ALT_REPLICATED");
        let opt_remote_dir = matches.get_one::<String>("REMOTE_DIR");
        let opt_local_dir = matches.get_one::<String>("LOCAL_DIR");

        let dataset_collection = FilesystemInfo::new(
            opt_alt_replicated,
            opt_remote_dir,
            opt_local_dir,
            opt_map_aliases,
            opt_alt_store,
            pwd.clone(),
        )?;

        let opt_json = matches.get_flag("JSON");

        let mut print_mode = if matches.get_flag("CSV") {
            PrintMode::Raw(RawMode::Csv)
        } else if matches.get_flag("ZEROS") {
            PrintMode::Raw(RawMode::Zero)
        } else if matches.get_flag("RAW") {
            PrintMode::Raw(RawMode::Newline)
        } else if matches.get_flag("NOT_SO_PRETTY") {
            PrintMode::Formatted(FormattedMode::NotPretty)
        } else {
            PrintMode::Formatted(FormattedMode::Default)
        };

        let opt_bulk_exclusion = if matches.get_flag("NO_LIVE") {
            Some(BulkExclusion::NoLive)
        } else if matches.get_flag("NO_SNAP") {
            Some(BulkExclusion::NoSnap)
        } else {
            None
        };

        if let Some(BulkExclusion::NoSnap) = opt_bulk_exclusion {
            if let PrintMode::Formatted(FormattedMode::Default) = print_mode {
                return HttmError::new("NO_SNAP is only available if RAW or ZEROS are specified.")
                    .into();
            }
        }

        // force a raw mode if one is not set for no_snap mode
        let opt_one_filesystem = matches.get_flag("ONE_FILESYSTEM");
        let opt_recursive = matches.get_flag("RECURSIVE");

        let opt_exact = matches.get_flag("EXACT");
        let opt_no_filter = matches.get_flag("NO_FILTER");
        let opt_no_hidden = matches.get_flag("FILTER_HIDDEN");
        let opt_no_clones =
            matches.get_flag("NO_CLONES") || std::env::var_os("HTTM_NO_CLONE").is_some();

        let opt_last_snap = match matches
            .get_one::<String>("LAST_SNAP")
            .map(|inner| inner.as_str())
        {
            Some("" | "any") => Some(LastSnapMode::Any),
            Some("none" | "without") => Some(LastSnapMode::Without),
            Some("ditto") => Some(LastSnapMode::DittoOnly),
            Some("no-ditto-inclusive") => Some(LastSnapMode::NoDittoInclusive),
            Some("no-ditto-exclusive" | "no-ditto") => Some(LastSnapMode::NoDittoExclusive),
            _ => None,
        };

        let opt_num_versions = match matches
            .get_one::<String>("NUM_VERSIONS")
            .map(|inner| inner.as_str())
        {
            Some("" | "all") => Some(NumVersionsMode::AllNumerals),
            Some("graph") => Some(NumVersionsMode::AllGraph),
            Some("single") => Some(NumVersionsMode::SingleAll),
            Some("single-no-snap") => Some(NumVersionsMode::SingleNoSnap),
            Some("single-with-snap") => Some(NumVersionsMode::SingleWithSnap),
            Some("multiple") => Some(NumVersionsMode::Multiple),
            _ => None,
        };

        if matches!(opt_num_versions, Some(NumVersionsMode::AllGraph))
            && !matches!(print_mode, PrintMode::Formatted(FormattedMode::Default))
        {
            return HttmError::new("The NUM_VERSIONS graph mode and the RAW or ZEROS display modes are an invalid combination.").into();
        }

        let opt_mount_display = match matches
            .get_one::<String>("FILE_MOUNT")
            .map(|inner| inner.as_str())
        {
            Some("" | "mount" | "target" | "directory") => Some(MountDisplay::Target),
            Some("source" | "device" | "dataset") => Some(MountDisplay::Source),
            Some("relative-path" | "relative" | "relpath") => Some(MountDisplay::RelativePath),
            _ => None,
        };

        let opt_preview = match matches
            .get_one::<String>("PREVIEW")
            .map(|inner| inner.as_str())
        {
            Some("" | "default") => Some("default".to_owned()),
            Some(user_defined) => Some(user_defined.to_string()),
            None => None,
        };

        let mut opt_deleted_mode = match matches
            .get_one::<String>("DELETED")
            .map(|inner| inner.as_str())
        {
            Some("" | "all") => Some(DeletedMode::All),
            Some("single" | "one") => Some(DeletedMode::DepthOfOne),
            Some("only") => Some(DeletedMode::Only),
            _ => None,
        };

        let opt_select_mode = matches.get_one::<String>("SELECT");
        let opt_restore_mode = matches
            .get_one::<String>("RESTORE")
            .or_else(|| matches.get_one::<String>("COPY"));

        let opt_interactive_mode = if let Some(var_restore_mode) = opt_restore_mode {
            let mut restore_mode = var_restore_mode.to_string();

            if let Ok(env_restore_mode) = std::env::var("HTTM_RESTORE_MODE") {
                restore_mode = env_restore_mode;
            }

            match restore_mode.as_str() {
                "guard" => Some(InteractiveMode::Restore(RestoreMode::Overwrite(
                    RestoreSnapGuard::Guarded,
                ))),
                "overwrite" | "yolo" => Some(InteractiveMode::Restore(RestoreMode::Overwrite(
                    RestoreSnapGuard::NotGuarded,
                ))),
                "copy-and-preserve" | "preserve" => {
                    Some(InteractiveMode::Restore(RestoreMode::CopyAndPreserve))
                }
                _ => Some(InteractiveMode::Restore(RestoreMode::CopyOnly)),
            }
        } else if opt_select_mode.is_some() || opt_preview.is_some() {
            match opt_select_mode.map(|inner| inner.as_str()) {
                Some("contents") => Some(InteractiveMode::Select(SelectMode::Contents)),
                Some("preview") => Some(InteractiveMode::Select(SelectMode::Preview)),
                Some(_) | None => Some(InteractiveMode::Select(SelectMode::Path)),
            }
        // simply enable browse mode -- if deleted mode not enabled but recursive search is specified,
        // that is, if delete recursive search is not specified, don't error out, let user browse
        } else if matches.get_flag("BROWSE") || (opt_recursive && opt_deleted_mode.is_none()) {
            Some(InteractiveMode::Browse)
        } else {
            None
        };

        let dedup_by = match matches
            .get_one::<String>("DEDUP_BY")
            .map(|inner| inner.as_str())
        {
            _ if matches.get_flag("PRUNE") => DedupBy::Disable,
            Some("all" | "no-filter" | "disable") => DedupBy::Disable,
            Some("contents") => DedupBy::Contents,
            Some("suspect") => DedupBy::Suspect,
            Some("metadata" | _) => DedupBy::Metadata,
            _ if matches.contains_id("LIST_SNAPS") => DedupBy::Disable,
            None => DedupBy::Metadata,
        };

        if opt_no_hidden && !opt_recursive && opt_interactive_mode.is_none() {
            return HttmError::new(
                "FILTER_HIDDEN is only available if either an interactive mode or recursive mode is specified.",
            )
            .into();
        }

        // if in last snap and select mode we will want to return a raw value,
        // better to have this here. It's more confusing if we work this logic later, I think.
        if opt_last_snap.is_some()
            && matches!(opt_interactive_mode, Some(InteractiveMode::Select(_)))
        {
            print_mode = PrintMode::Raw(RawMode::Newline)
        }

        let opt_snap_file_mount =
            if let Some(requested_snapshot_suffix) = matches.get_one::<String>("SNAPSHOT") {
                if requested_snapshot_suffix == &"httmSnapFileMount" {
                    Some(requested_snapshot_suffix.to_owned())
                } else if requested_snapshot_suffix.contains(char::is_whitespace) {
                    return HttmError::new(
                        "httm will only accept snapshot suffixes which don't contain whitespace",
                    )
                    .into();
                } else {
                    Some(requested_snapshot_suffix.to_owned())
                }
            } else {
                None
            };

        let opt_snap_mode_filters = if matches.contains_id("LIST_SNAPS") {
            // allow selection of snaps to prune in prune mode
            let select_mode = matches!(opt_interactive_mode, Some(InteractiveMode::Select(_)));

            if !matches.get_flag("PRUNE") && select_mode {
                eprintln!("Select mode for listed snapshots only available in PRUNE mode.")
            }

            match matches.get_one::<String>("LIST_SNAPS") {
                Some(value) if !value.is_empty() => Some(Self::snap_filters(value, select_mode)?),
                _ => Some(ListSnapsFilters {
                    select_mode,
                    omit_num_snaps: 0usize,
                    name_filters: None,
                }),
            }
        } else {
            None
        };

        let mut exec_mode = if let Some(full_snap_name) = matches.get_one::<String>("ROLL_FORWARD")
        {
            ExecMode::RollForward(full_snap_name.to_owned())
        } else if let Some(num_versions_mode) = opt_num_versions {
            ExecMode::NumVersions(num_versions_mode)
        } else if let Some(mount_display) = opt_mount_display {
            ExecMode::MountsForFiles(mount_display)
        } else if matches.get_flag("PRUNE") {
            ExecMode::Prune(opt_snap_mode_filters)
        } else if opt_snap_mode_filters.is_some() {
            ExecMode::SnapsForFiles(opt_snap_mode_filters)
        } else if let Some(requested_snapshot_suffix) = opt_snap_file_mount {
            ExecMode::SnapFileMount(requested_snapshot_suffix.to_string())
        } else if let Some(interactive_mode) = opt_interactive_mode {
            ExecMode::Interactive(interactive_mode)
        } else if opt_deleted_mode.is_some() {
            let progress_bar: ProgressBar = indicatif::ProgressBar::new_spinner();
            ExecMode::NonInteractiveRecursive(progress_bar)
        } else {
            ExecMode::BasicDisplay
        };

        if opt_no_filter && !opt_recursive {
            return HttmError::new("NO_FILTER only available when recursive search is enabled.")
                .into();
        }

        // paths are immediately converted to our PathData struct
        let opt_os_values = matches.get_many::<PathBuf>("INPUT_FILES");

        let paths: Vec<PathData> = Self::paths(opt_os_values, &exec_mode, &pwd)?;

        let opt_lazy = matches.get_flag("LAZY_SNAP_EVAL")
            || (matches!(exec_mode, ExecMode::BasicDisplay) && paths.len() == 1);

        // for exec_modes in which we can only take a single directory, process how we handle those here
        let opt_requested_dir: Option<PathBuf> =
            Self::opt_requested_dir(&mut exec_mode, &mut opt_deleted_mode, &paths, &pwd)?;

        if opt_one_filesystem && opt_requested_dir.is_none() {
            return HttmError::new("ONE_FILESYSTEM requires a requested path for RECURSIVE search")
                .into();
        }

        // doesn't make sense to follow symlinks when you're searching the whole system,
        // so we disable our bespoke "when to traverse symlinks" algo here, or if requested.
        let opt_no_traverse = matches.get_flag("NO_TRAVERSE") || {
            if let Some(user_requested_dir) = opt_requested_dir.as_ref() {
                user_requested_dir.as_path() == ROOT_PATH.as_path()
            } else {
                false
            }
        };

        if !matches!(opt_deleted_mode, None | Some(DeletedMode::All)) && !opt_recursive {
            return HttmError::new(
                "Deleted modes other than \"all\" require recursive mode is enabled. Quitting.",
            )
            .into();
        }

        let opt_omit_ditto = matches.get_flag("OMIT_DITTO");

        // opt_omit_identical doesn't make sense in Display Recursive mode as no live files will exists?
        if opt_omit_ditto && matches!(exec_mode, ExecMode::NonInteractiveRecursive(_)) {
            return HttmError::new(
                "OMIT_DITTO not available when a deleted recursive search is specified. Quitting.",
            )
            .into();
        }

        if opt_last_snap.is_some() && matches!(exec_mode, ExecMode::NonInteractiveRecursive(_)) {
            return HttmError::new("LAST_SNAP is not available in Display Recursive Mode.").into();
        }

        let config = Config {
            paths,
            opt_bulk_exclusion,
            opt_recursive,
            opt_exact,
            opt_debug,
            opt_no_traverse,
            opt_omit_ditto,
            opt_no_hidden,
            opt_no_filter,
            opt_last_snap,
            opt_preview,
            opt_json,
            opt_one_filesystem,
            opt_no_clones,
            opt_lazy,
            dedup_by,
            requested_utc_offset,
            exec_mode,
            print_mode,
            opt_deleted_mode,
            dataset_collection,
            pwd,
            opt_requested_dir,
        };

        Ok(config)
    }
}

impl Config {
    pub fn new() -> HttmResult<Self> {
        let arg_matches = parse_args();
        let config = Config::try_from(&arg_matches)?;
        if config.opt_debug {
            eprintln!("{config:#?}");
        }
        Ok(config)
    }

    #[inline(always)]
    pub fn paths(
        opt_os_values: Option<ValuesRef<'_, PathBuf>>,
        exec_mode: &ExecMode,
        pwd: &Path,
    ) -> HttmResult<Vec<PathData>> {
        let mut paths = if let Some(input_files) = opt_os_values {
            input_files
                .par_bridge()
                // canonicalize() on a deleted relative path will not exist,
                // so we have to join with the pwd to make a path that
                // will exist on a snapshot
                .map(PathData::from)
                .map(|pd| {
                    // but what about snapshot paths?
                    // here we strip the additional snapshot VFS bits and make them look like live versions
                    match ZfsSnapPathGuard::new(&pd) {
                        Some(spd) if !matches!(exec_mode, ExecMode::MountsForFiles(_)) => spd
                            .live_path()
                            .map(|path| path.into())
                            .unwrap_or_else(|| pd),
                        _ => pd,
                    }
                })
                .collect()
        } else {
            match exec_mode {
                // setting pwd as the path, here, keeps us from waiting on stdin when in certain modes
                //  is more like Interactive and NonInteractiveRecursive in this respect in requiring only one
                // input, and waiting on one input from stdin is pretty silly
                ExecMode::Interactive(_)
                | ExecMode::NonInteractiveRecursive(_)
                | ExecMode::RollForward(_) => {
                    vec![PathData::from(pwd)]
                }
                ExecMode::BasicDisplay
                | ExecMode::Preview
                | ExecMode::SnapFileMount(_)
                | ExecMode::Prune(_)
                | ExecMode::MountsForFiles(_)
                | ExecMode::SnapsForFiles(_)
                | ExecMode::NumVersions(_) => Self::read_stdin()?,
            }
        };

        // deduplicate path_data and sort if in display mode --
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

    pub fn read_stdin() -> HttmResult<Vec<PathData>> {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let mut buffer = Vec::new();
        stdin.read_to_end(&mut buffer)?;

        let buffer_string = std::str::from_utf8(&buffer)?;

        let broken_string = if buffer_string.contains(['\n', '\0']) {
            // always split on newline or null char, if available
            buffer_string
                .split(&['\n', '\0'])
                .filter(|s| !s.is_empty())
                .map(PathData::from)
                .collect()
        } else if buffer_string.contains('\"') {
            buffer_string
                .split('\"')
                // unquoted paths should have excess whitespace trimmed
                .map(str::trim)
                // remove any empty strings
                .filter(|s| !s.is_empty())
                .map(PathData::from)
                .collect()
        } else {
            buffer_string
                .split_ascii_whitespace()
                .filter(|s| !s.is_empty())
                .map(PathData::from)
                .collect()
        };

        Ok(broken_string)
    }

    #[inline(always)]
    pub fn opt_requested_dir(
        exec_mode: &mut ExecMode,
        deleted_mode: &mut Option<DeletedMode>,
        paths: &[PathData],
        pwd: &Path,
    ) -> HttmResult<Option<PathBuf>> {
        let res = match exec_mode {
            ExecMode::Interactive(_) | ExecMode::NonInteractiveRecursive(_) => {
                match paths.len() {
                    0 => Some(pwd.to_path_buf()),
                    // safe to index as we know the paths len is 1
                    1 if paths[0].opt_file_type().is_some_and(|ft| {
                        ft.is_dir()
                            || (ft.is_symlink()
                                && read_link(paths[0].path())
                                    .ok()
                                    .is_some_and(|link_target| link_target.is_dir()))
                    }) =>
                    {
                        Some(paths[0].path().to_path_buf())
                    }
                    // handle non-directories
                    1 => {
                        match exec_mode {
                            ExecMode::Interactive(interactive_mode) => {
                                match interactive_mode {
                                    InteractiveMode::Browse => {
                                        // doesn't make sense to have a non-dir in these modes
                                        return HttmError::new(
                                                    "Path specified is not a directory, and therefore not suitable for browsing.",
                                                )
                                                .into();
                                    }
                                    InteractiveMode::Restore(_) | InteractiveMode::Select(_) => {
                                        // non-dir file will just cause us to skip the lookup phase
                                        None
                                    }
                                }
                            }
                            // disable NonInteractiveRecursive when path given is not a directory
                            // switch to a standard Display mode
                            ExecMode::NonInteractiveRecursive(_) => {
                                eprintln!(
                                    "WARN: Disabling non-interactive recursive mode as requested directory either does not exist or is not a directory.  \
                                Switching to display mode."
                                );
                                *exec_mode = ExecMode::BasicDisplay;
                                *deleted_mode = None;
                                None
                            }
                            _ => unreachable!(),
                        }
                    }
                    n if n > 1 => return HttmError::new(
                        "May only specify one path in the display recursive or interactive modes.",
                    )
                    .into(),
                    _ => {
                        unreachable!()
                    }
                }
            }

            ExecMode::BasicDisplay
            | ExecMode::Preview
            | ExecMode::RollForward(_)
            | ExecMode::SnapFileMount(_)
            | ExecMode::Prune(_)
            | ExecMode::MountsForFiles(_)
            | ExecMode::SnapsForFiles(_)
            | ExecMode::NumVersions(_) => {
                // in non-interactive mode / display mode, requested dir is just a file
                // like every other file and pwd must be the requested working dir.
                None
            }
        };
        Ok(res)
    }

    pub fn snap_filters(values: &str, select_mode: bool) -> HttmResult<ListSnapsFilters> {
        let mut raw = values.trim_end().split(',');
        let opt_number = raw.next();
        let mut rest: Vec<&str> = raw.collect();

        let omit_num_snaps = if let Some(value) = opt_number {
            match value.parse::<usize>() {
                Ok(number) => number,
                Err(_) => {
                    rest = values.trim_end().split(',').collect();
                    0usize
                }
            }
        } else {
            0usize
        };

        let name_filters = if !rest.is_empty() {
            if rest.len() == 1usize && rest.index(0) == &"none" {
                None
            } else if rest.len() == 1usize && rest.index(0) == &"native" {
                Some(
                    NATIVE_SNAP_SUFFIXES
                        .into_iter()
                        .map(|name| name.to_owned())
                        .collect(),
                )
            } else {
                Some(rest.iter().map(|item| item.to_string()).collect())
            }
        } else {
            None
        };

        Ok(ListSnapsFilters {
            select_mode,
            omit_num_snaps,
            name_filters,
        })
    }
}
