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
use crate::config::generate::{PrintMode, RawMode};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::data::selection::SelectionCandidate;
use crate::library::results::{HttmError, HttmResult};
use lscolors::{LsColors, Style};
use nu_ansi_term::AnsiString;
use std::borrow::Cow;
use std::cell::RefMut;
use std::fs::FileType;
use std::hash::Hash;
use std::io::Write;
use std::iter::Iterator;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;
use time::{OffsetDateTime, UtcOffset, format_description};
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

pub fn dir_was_previously_listed(
    entry: &BasicDirEntryInfo,
    opt_path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
) -> Option<bool> {
    let file_id = UniqueInode::new(entry)?;

    let path_map = opt_path_map?;

    Some(!path_map.insert(file_id))
}

pub struct UniqueInode {
    ino: u64,
    dev: u64,
}

impl UniqueInode {
    fn new(entry: &BasicDirEntryInfo) -> Option<Self> {
        // deref if needed!
        let entry_metadata = match entry.opt_filetype() {
            Some(ft) if ft.is_symlink() => entry.path().metadata().ok(),
            Some(_) => entry.opt_metadata(),
            None => entry.path().metadata().ok(),
        };

        Some(Self {
            ino: entry_metadata.as_ref()?.ino(),
            dev: entry_metadata.as_ref()?.dev(),
        })
    }
}

impl PartialEq for UniqueInode {
    fn eq(&self, other: &Self) -> bool {
        self.ino == other.ino && self.dev == other.dev
    }
}

impl Eq for UniqueInode {}

impl Hash for UniqueInode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.ino.hash(state);
        self.dev.hash(state);
    }
}

// is this path/dir_entry something we should count as a directory for our purposes?
pub fn httm_is_dir<'a, T>(
    entry: &'a T,
    opt_path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
) -> bool
where
    T: HttmIsDir<'a> + ?Sized,
{
    match entry.file_type() {
        Ok(file_type) => match file_type {
            file_type if file_type.is_dir() => true,
            file_type if file_type.is_file() => false,
            file_type if file_type.is_symlink() => {
                // canonicalize will read_link/resolve the link for us
                match entry.path().read_link() {
                    Ok(link_target) if !link_target.is_dir() => false,
                    Ok(link_target) => {
                        let entry = BasicDirEntryInfo::new(&link_target, None);

                        match dir_was_previously_listed(&entry, opt_path_map) {
                            Some(dir_was_previously_listed) if dir_was_previously_listed => false,
                            Some(_) => true,
                            None => false,
                        }
                    }
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
    fn httm_is_dir(
        &self,
        path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
    ) -> bool;
    fn file_type(&self) -> Result<FileType, std::io::Error>;
    fn path(&'a self) -> &'a Path;
}

impl<T: AsRef<Path>> HttmIsDir<'_> for T {
    fn httm_is_dir(
        &self,
        path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
    ) -> bool {
        httm_is_dir(self, path_map)
    }
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        Ok(self.as_ref().symlink_metadata()?.file_type())
    }
    fn path(&self) -> &Path {
        self.as_ref()
    }
}

impl<'a> HttmIsDir<'a> for PathData {
    fn httm_is_dir(
        &self,
        path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
    ) -> bool {
        httm_is_dir(self, path_map)
    }
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        Ok(self
            .opt_file_type()
            .ok_or_else(|| std::io::ErrorKind::NotFound)?)
    }
    fn path(&'a self) -> &'a Path {
        &self.path()
    }
}

impl<'a> HttmIsDir<'a> for BasicDirEntryInfo {
    fn httm_is_dir(
        &self,
        path_map: Option<&mut RefMut<'_, hashbrown::HashSet<UniqueInode>>>,
    ) -> bool {
        httm_is_dir(self, path_map)
    }
    fn file_type(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        self.opt_filetype()
            .copied()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn path(&'a self) -> &'a Path {
        &self.path()
    }
}

pub static ENV_LS_COLORS: LazyLock<LsColors> =
    LazyLock::new(|| LsColors::from_env().unwrap_or_default());
static BASE_STYLE: LazyLock<nu_ansi_term::Style> = LazyLock::new(|| nu_ansi_term::Style::default());
static PHANTOM_STYLE: LazyLock<nu_ansi_term::Style> = LazyLock::new(|| BASE_STYLE.dimmed());

pub fn paint_string<'a, T>(item: &'a T) -> AnsiString<'a>
where
    T: PaintString,
{
    let display_name = item.name();

    match item
        .ls_style()
        .map(|style| Style::to_nu_ansi_term_style(&style))
    {
        Some(ansi_style) if !item.is_phantom() => ansi_style.paint(display_name),
        None if !item.is_phantom() => BASE_STYLE.paint(display_name),
        _ => PHANTOM_STYLE.paint(display_name),
    }
}

pub trait PaintString {
    fn paint_string<'a>(&'a self) -> AnsiString<'a>;
    fn ls_style(&self) -> Option<lscolors::style::Style>;
    fn is_phantom(&self) -> bool;
    fn name(&self) -> Cow<'_, str>;
}

impl PaintString for PathData {
    fn paint_string<'a>(&'a self) -> AnsiString<'a> {
        paint_string(self)
    }
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

impl PaintString for SelectionCandidate {
    fn paint_string<'a>(&'a self) -> AnsiString<'a> {
        paint_string(self)
    }
    fn ls_style(&self) -> Option<lscolors::style::Style> {
        ENV_LS_COLORS.style_for(self).copied()
    }
    fn is_phantom(&self) -> bool {
        self.opt_filetype().is_none()
    }
    fn name(&self) -> Cow<'_, str> {
        let mut display_name = self.display_name().to_string();

        match self.opt_filetype() {
            Some(ft) if !ft.is_symlink() && !ft.is_dir() => (),
            Some(ft) if ft.is_dir() && display_name != "/" => display_name.push('/'),
            Some(ft) if ft.is_symlink() => match std::fs::read_link(self.path()).ok() {
                Some(link_target) => {
                    let link_name = format!(" -> {}", link_target.to_string_lossy());
                    display_name.push_str(&link_name);
                }
                None => {
                    display_name.push_str(" -> ?");
                }
            },
            _ => (),
        }

        Cow::Owned(display_name)
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
