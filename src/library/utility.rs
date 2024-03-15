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

use crate::config::generate::PrintMode;
use crate::data::paths::{BasicDirEntryInfo, PathData, PathMetadata, PHANTOM_DATE};
use crate::data::selection::SelectionCandidate;
use crate::library::results::{HttmError, HttmResult};

use crate::parse::mounts::FilesystemType;
use crate::{BTRFS_SNAPPER_HIDDEN_DIRECTORY, GLOBAL_CONFIG, ZFS_SNAPSHOT_DIRECTORY};
use crossbeam_channel::{Receiver, TryRecvError};
use lscolors::{Colorable, LsColors, Style};
use nu_ansi_term::Style as AnsiTermStyle;
use number_prefix::NumberPrefix;
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::fs::FileType;
use std::io::Write;
use std::iter::Iterator;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use time::{format_description, OffsetDateTime, UtcOffset};

#[cfg(feature = "setpriority")]
#[cfg(target_os = "linux")]
#[cfg(target_env = "gnu")]
#[allow(dead_code)]
pub enum ThreadPriorityType {
    Process = 0,
    PGroup = 1,
    User = 2,
}

#[cfg(feature = "setpriority")]
#[cfg(target_os = "linux")]
#[cfg(target_env = "gnu")]
impl ThreadPriorityType {
    // nice calling thread to a specified level
    pub fn nice_thread(self, opt_tid: Option<u32>, priority_level: i32) -> HttmResult<()> {
        let tid = opt_tid.unwrap_or_else(|| std::process::id());

        // #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        // unsafe {
        //     let _ = libc::setpriority(priority_type as i32, tid, priority_level);
        // };

        unsafe {
            let priority_type = self as u32;
            let _ = libc::setpriority(priority_type, tid, priority_level);
        }

        Ok(())
    }
}

#[cfg(feature = "malloc_trim")]
#[cfg(target_os = "linux")]
#[cfg(target_env = "gnu")]
pub fn malloc_trim() {
    unsafe {
        let _ = libc::malloc_trim(0);
    }
}

pub fn user_has_effective_root(msg: &str) -> HttmResult<()> {
    if !nix::unistd::geteuid().is_root() {
        let err = format!("Superuser privileges are required to execute: {}.", msg);
        return Err(HttmError::new(&err).into());
    }

    Ok(())
}

pub fn delimiter() -> char {
    if matches!(GLOBAL_CONFIG.print_mode, PrintMode::RawZero) {
        '\0'
    } else {
        '\n'
    }
}

pub enum Never {}

pub fn is_channel_closed(chan: &Receiver<Never>) -> bool {
    match chan.try_recv() {
        Ok(never) => match never {},
        Err(TryRecvError::Disconnected) => true,
        Err(TryRecvError::Empty) => false,
    }
}

const TMP_SUFFIX: &str = ".tmp";

pub fn make_tmp_path(path: &Path) -> PathBuf {
    let path_string = path.to_string_lossy().to_string();
    let res = path_string + TMP_SUFFIX;
    PathBuf::from(res)
}

pub fn find_common_path<I, P>(paths: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut path_iter = paths.into_iter();
    let initial_value = path_iter.next()?.as_ref().to_path_buf();

    path_iter.try_fold(initial_value, |acc, path| cmp_path(acc, path))
}

fn cmp_path<A: AsRef<Path>, B: AsRef<Path>>(a: A, b: B) -> Option<PathBuf> {
    // skip the root dir,
    let a_components = a.as_ref().components();
    let b_components = b.as_ref().components();

    let common_path: PathBuf = a_components
        .zip(b_components)
        .take_while(|(a_path, b_path)| a_path == b_path)
        .map(|(a_path, _b_path)| a_path)
        .collect();

    if common_path.components().count() > 1 {
        Some(common_path)
    } else {
        None
    }
}

pub fn print_output_buf(output_buf: &str) -> HttmResult<()> {
    // mutex keeps threads from writing over each other
    let out = std::io::stdout();
    let mut out_locked = out.lock();
    out_locked.write_all(output_buf.as_bytes())?;
    out_locked.flush().map_err(std::convert::Into::into)
}

// is this path/dir_entry something we should count as a directory for our purposes?
pub fn httm_is_dir<'a, T>(entry: &'a T) -> bool
where
    T: HttmIsDir<'a> + ?Sized,
{
    let path = entry.path();

    match entry.filetype() {
        Ok(file_type) => match file_type {
            file_type if file_type.is_dir() => true,
            file_type if file_type.is_file() => false,
            file_type if file_type.is_symlink() => {
                // canonicalize will read_link/resolve the link for us
                match path.canonicalize() {
                    Ok(link_target) if !link_target.is_dir() => false,
                    Ok(link_target) => path.ancestors().all(|ancestor| ancestor != link_target),
                    // we get an error? still pass the path on, as we get a good path from the dir entry
                    _ => false,
                }
            }
            // char, block, etc devices(?), None/Errs are not dirs, and we have a good path to pass on, so false
            _ => false,
        },
        _ => false,
    }
}
pub trait HttmIsDir<'a> {
    fn httm_is_dir(&self) -> bool;
    fn filetype(&self) -> Result<FileType, std::io::Error>;
    fn path(&'a self) -> &'a Path;
}

impl<T: AsRef<Path>> HttmIsDir<'_> for T {
    fn httm_is_dir(&self) -> bool {
        httm_is_dir(self)
    }
    fn filetype(&self) -> Result<FileType, std::io::Error> {
        Ok(self.as_ref().symlink_metadata()?.file_type())
    }
    fn path(&self) -> &Path {
        self.as_ref()
    }
}

impl<'a> HttmIsDir<'a> for PathData {
    fn httm_is_dir(&self) -> bool {
        httm_is_dir(self)
    }
    fn filetype(&self) -> Result<FileType, std::io::Error> {
        Ok(self.path_buf.symlink_metadata()?.file_type())
    }
    fn path(&'a self) -> &'a Path {
        &self.path_buf
    }
}

impl<'a> HttmIsDir<'a> for BasicDirEntryInfo {
    fn httm_is_dir(&self) -> bool {
        httm_is_dir(self)
    }
    fn filetype(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        self.file_type
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn path(&'a self) -> &'a Path {
        &self.path
    }
}

static ENV_LS_COLORS: Lazy<LsColors> = Lazy::new(|| LsColors::from_env().unwrap_or_default());
static PHANTOM_STYLE: Lazy<AnsiTermStyle> = Lazy::new(|| {
    Style::to_nu_ansi_term_style(
        &Style::from_ansi_sequence("38;2;250;200;200;1;0").unwrap_or_default(),
    )
});

pub fn paint_string<T>(path: T, display_name: &str) -> Cow<str>
where
    T: PaintString,
{
    if path.is_phantom() {
        // paint all other phantoms/deleted files the same color, light pink
        return Cow::Owned(PHANTOM_STYLE.paint(display_name).to_string());
    }

    if let Some(style) = path.ls_style() {
        let ansi_style: &AnsiTermStyle = &Style::to_nu_ansi_term_style(style);
        return Cow::Owned(ansi_style.paint(display_name).to_string());
    }

    // if a non-phantom file that should not be colored (sometimes -- your regular files)
    // or just in case if all else fails, don't paint and return string
    Cow::Borrowed(display_name)
}

pub trait PaintString {
    fn ls_style(&self) -> Option<&'_ lscolors::style::Style>;
    fn is_phantom(&self) -> bool;
}

impl PaintString for &PathData {
    fn ls_style(&self) -> Option<&lscolors::style::Style> {
        ENV_LS_COLORS.style_for_path(&self.path_buf)
    }
    fn is_phantom(&self) -> bool {
        self.metadata.is_none()
    }
}

impl PaintString for &SelectionCandidate {
    fn ls_style(&self) -> Option<&lscolors::style::Style> {
        ENV_LS_COLORS.style_for(self)
    }
    fn is_phantom(&self) -> bool {
        self.file_type().is_none()
    }
}

pub fn fs_type_from_hidden_dir(dataset_mount: &Path) -> Option<FilesystemType> {
    // set fstype, known by whether there is a ZFS hidden snapshot dir in the root dir
    if dataset_mount
        .join(ZFS_SNAPSHOT_DIRECTORY)
        .symlink_metadata()
        .is_ok()
    {
        Some(FilesystemType::Zfs)
    } else if dataset_mount
        .join(BTRFS_SNAPPER_HIDDEN_DIRECTORY)
        .symlink_metadata()
        .is_ok()
    {
        Some(FilesystemType::Btrfs(None))
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateFormat {
    Display,
    Timestamp,
}

static DATE_FORMAT_DISPLAY: &str =
    "[weekday repr:short] [month repr:short] [day] [hour]:[minute]:[second] [year]";
static DATE_FORMAT_TIMESTAMP: &str = "[year]-[month]-[day]-[hour]:[minute]:[second]";

pub fn date_string(
    utc_offset: UtcOffset,
    system_time: &SystemTime,
    date_format: DateFormat,
) -> String {
    let date_time: OffsetDateTime = (*system_time).into();

    let parsed_format = format_description::parse(date_string_format(&date_format))
        .expect("timestamp date format is invalid");

    let raw_string = date_time
        .to_offset(utc_offset)
        .format(&parsed_format)
        .expect("timestamp date format could not be applied to the date supplied");

    if utc_offset == UtcOffset::UTC {
        return match &date_format {
            DateFormat::Timestamp => raw_string + "_UTC",
            DateFormat::Display => raw_string + " UTC",
        };
    }

    raw_string
}

fn date_string_format<'a>(format: &DateFormat) -> &'a str {
    match format {
        DateFormat::Display => DATE_FORMAT_DISPLAY,
        DateFormat::Timestamp => DATE_FORMAT_TIMESTAMP,
    }
}

pub fn display_human_size(size: u64) -> String {
    let size = size as f64;

    match NumberPrefix::binary(size) {
        NumberPrefix::Standalone(bytes) => format!("{bytes} bytes"),
        NumberPrefix::Prefixed(prefix, n) => format!("{n:.1} {prefix}B"),
    }
}

pub fn is_metadata_same<T>(src: T, dst: T) -> HttmResult<()>
where
    T: ComparePathMetadata,
{
    if src.opt_metadata().is_none() {
        let msg = format!("WARN: Metadata not found: {:?}", src.path());
        return Err(HttmError::new(&msg).into());
    }

    if src.path().is_symlink() && (src.path().read_link().ok() != dst.path().read_link().ok()) {
        let msg = format!("WARN: Symlink do not match: {:?}", src.path());
        return Err(HttmError::new(&msg).into());
    }

    if src.opt_metadata() != dst.opt_metadata() {
        let msg = format!(
            "WARN: Metadata mismatch: {:?} !-> {:?}",
            src.path(),
            dst.path()
        );
        return Err(HttmError::new(&msg).into());
    }

    Ok(())
}

pub trait ComparePathMetadata {
    fn opt_metadata(&self) -> Option<PathMetadata>;
    fn path(&self) -> &Path;
}

impl<T: AsRef<Path>> ComparePathMetadata for T {
    fn opt_metadata(&self) -> Option<PathMetadata> {
        // never follow symlinks for comparison
        let opt_md = self.as_ref().symlink_metadata().ok();

        opt_md.map(|md| PathMetadata {
            size: md.len(),
            modify_time: md.modified().unwrap_or(PHANTOM_DATE),
        })
    }

    fn path(&self) -> &Path {
        self.as_ref()
    }
}

pub fn path_is_filter_dir(path: &Path) -> bool {
    GLOBAL_CONFIG
        .dataset_collection
        .filter_dirs
        .deref()
        .iter()
        .any(|filter_dir| path == filter_dir)
}

pub fn pwd() -> HttmResult<PathBuf> {
    if let Ok(pwd) = std::env::current_dir() {
        Ok(pwd)
    } else {
        Err(HttmError::new(
            "Working directory does not exist or your do not have permissions to access it.",
        )
        .into())
    }
}
