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

use crate::GLOBAL_CONFIG;
use crate::config::generate::{
    PrintMode,
    RawMode,
};
use crate::data::paths::{
    BasicDirEntryInfo,
    PathData,
};
use crate::data::selection::SelectionCandidate;
use crate::library::results::{
    HttmError,
    HttmResult,
};
use lscolors::{
    LsColors,
    Style,
};
use nu_ansi_term::AnsiString;
use std::borrow::Cow;
use std::fs::FileType;
use std::io::Write;
use std::iter::Iterator;
use std::path::{
    Path,
    PathBuf,
};
use std::sync::LazyLock;
use std::time::SystemTime;
use time::{
    OffsetDateTime,
    UtcOffset,
    format_description,
};
use unit_prefix::NumberPrefix;
use which::which;

pub fn pwd() -> HttmResult<PathBuf> {
    let Ok(pwd) = std::env::current_dir() else {
        return HttmError::new(
            "Working directory does not exist or your do not have permissions to access it.",
        )
        .into();
    };

    Ok(pwd)
}

pub fn get_mount_command() -> HttmResult<PathBuf> {
    which("mount").map_err(|_err| {
        HttmError::new(
            "'mount' command not be found. Make sure the command 'mount' is in your path.",
        )
        .into()
    })
}

pub fn get_btrfs_command() -> HttmResult<PathBuf> {
    which("btrfs").map_err(|_err| {
        HttmError::new("'btrfs' command not found. Make sure the command 'btrfs' is in your path.")
            .into()
    })
}

pub fn user_has_effective_root(msg: &str) -> HttmResult<()> {
    if !nix::unistd::geteuid().is_root() {
        let err = format!("Superuser privileges are required to execute: {}.", msg);
        return HttmError::new(&err).into();
    }

    Ok(())
}

pub fn delimiter() -> char {
    if let PrintMode::Raw(RawMode::Zero) = GLOBAL_CONFIG.print_mode {
        return '\0';
    }

    '\n'
}

// pub enum Never {}

// pub fn is_channel_closed(chan: &Receiver<Never>) -> bool {
//     match chan.try_recv() {
//         Ok(never) => match never {},
//         Err(TryRecvError::Disconnected) => true,
//         Err(TryRecvError::Empty) => false,
//     }
// }

pub fn make_tmp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp_httm")
}

pub fn find_common_path<I, P>(paths: I) -> Option<Box<Path>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut path_iter = paths.into_iter();
    let initial_value = path_iter.next()?.as_ref().to_path_buf();

    path_iter
        .try_fold(initial_value, |acc, path| cmp_path(acc, path))
        .map(|res| res.into_boxed_path())
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

pub trait HttmIsDir {
    fn file_type(&self) -> Result<FileType, std::io::Error>;
    fn path(&self) -> &Path;

    // is this path/dir_entry something we should count as a directory for our purposes?
    fn httm_is_dir<T>(&self) -> bool {
        match self.file_type() {
            Ok(file_type) => match file_type {
                file_type if file_type.is_dir() => true,
                file_type if file_type.is_file() => false,
                file_type if file_type.is_symlink() => {
                    // canonicalize will read_link/resolve the link for us
                    match self.path().read_link() {
                        Ok(link_target) if link_target.is_dir() => true,
                        Ok(_link_target) => false,
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
}

impl<T: AsRef<Path>> HttmIsDir for T {
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        Ok(self.as_ref().symlink_metadata()?.file_type())
    }
    fn path(&self) -> &Path {
        self.as_ref()
    }
}

impl HttmIsDir for PathData {
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        Ok(self
            .opt_file_type()
            .ok_or_else(|| std::io::ErrorKind::NotFound)?)
    }
    fn path(&self) -> &Path {
        &self.path()
    }
}

impl HttmIsDir for BasicDirEntryInfo {
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        self.opt_filetype()
            .copied()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn path(&self) -> &Path {
        self.path()
    }
}

pub static ENV_LS_COLORS: LazyLock<LsColors> =
    LazyLock::new(|| LsColors::from_env().unwrap_or_default());
static BASE_STYLE: LazyLock<nu_ansi_term::Style> = LazyLock::new(|| nu_ansi_term::Style::default());
static PHANTOM_STYLE: LazyLock<nu_ansi_term::Style> = LazyLock::new(|| BASE_STYLE.dimmed());

pub trait PaintString<'a> {
    fn ls_style(&self) -> Option<lscolors::style::Style>;
    fn is_phantom(&self) -> bool;
    fn name(&self) -> Cow<'_, str>;

    fn paint_string(&'a self) -> AnsiString<'a> {
        let display_name = self.name();

        if self.is_phantom() {
            return PHANTOM_STYLE.paint(display_name);
        }

        match self
            .ls_style()
            .map(|style| Style::to_nu_ansi_term_style(&style))
        {
            Some(ansi_style) => ansi_style.paint(display_name),
            None => BASE_STYLE.paint(display_name),
        }
    }
}

impl<'a> PaintString<'a> for PathData {
    fn ls_style(&self) -> Option<lscolors::style::Style> {
        self.opt_style()
    }
    fn is_phantom(&self) -> bool {
        self.opt_path_metadata().is_none()
    }
    fn name(&self) -> Cow<'_, str> {
        self.path().to_string_lossy()
    }
}

impl<'a> PaintString<'a> for SelectionCandidate {
    fn ls_style(&self) -> Option<lscolors::style::Style> {
        self.opt_style().copied()
    }
    fn is_phantom(&self) -> bool {
        self.opt_filetype().is_none()
    }
    fn name(&self) -> Cow<'_, str> {
        let display_name = self.display_name();

        match self.opt_filetype() {
            Some(ft) if ft.is_file() => display_name,
            Some(ft) if ft.is_dir() && display_name != "/" => {
                let mut res = display_name.to_string();
                res.push('/');
                Cow::Owned(res)
            }
            Some(ft) if ft.is_symlink() => match std::fs::read_link(self.path()).ok() {
                Some(link_target) => {
                    let link_name = format!(" -> {}", link_target.to_string_lossy());
                    let mut res = display_name.to_string();
                    res.push_str(&link_name);
                    Cow::Owned(res)
                }
                None => {
                    let mut res = display_name.to_string();
                    res.push_str(" -> ?");
                    Cow::Owned(res)
                }
            },
            _ => display_name,
        }
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

pub fn is_metadata_same<P>(src: P, dst: P) -> HttmResult<()>
where
    P: AsRef<Path>,
{
    let src_pd = PathData::without_styling(src.as_ref(), None);
    let dst_pd = PathData::without_styling(dst.as_ref(), None);

    if src_pd.opt_path_metadata().is_none() {
        let description = format!("Metadata not found: {:?}", src.as_ref());
        return HttmError::from(description).into();
    }

    if dst_pd.opt_path_metadata().is_none() {
        let description = format!("Metadata not found: {:?}", dst.as_ref());
        return HttmError::from(description).into();
    }

    if src.as_ref().is_symlink() && (src.as_ref().read_link().ok() != dst.as_ref().read_link().ok())
    {
        let description = format!(
            "Symlink targets do not match: {:?} -> {:?}",
            src.as_ref(),
            dst.as_ref()
        );
        return HttmError::from(description).into();
    }

    if src_pd.opt_path_metadata() != dst_pd.opt_path_metadata() {
        let description = format!(
            "Metadata mismatch: {:?}::{:?} !-> {:?}::{:?}",
            src.path(),
            src_pd.opt_path_metadata(),
            dst.path(),
            dst_pd.opt_path_metadata()
        );
        return HttmError::from(description).into();
    }

    Ok(())
}
