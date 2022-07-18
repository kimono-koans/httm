// (c) Robert Swinford <robert.swinford<...at...>gmail.com>
//
// For the full copyright and license information, please view the LICENSE file
// that was distributed with this source code.

use std::{
    borrow::Cow,
    cmp,
    error::Error,
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    fs::{copy, create_dir_all, read_dir, symlink_metadata, DirEntry, FileType, Metadata},
    io::{self, Read, Write},
    path::{Component::RootDir, Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use lscolors::{LsColors, Style};

use crate::interactive::SelectionCandidate;
use crate::{PHANTOM_DATE, PHANTOM_SIZE};

pub fn timestamp_file(system_time: &SystemTime) -> String {
    let date_time: DateTime<Local> = (*system_time).into();
    format!("{}", date_time.format("%b-%d-%Y-%H:%M:%S"))
}

pub fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if PathBuf::from(src).is_dir() {
        create_dir_all(&dst)?;
        for entry in read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                copy_recursive(&entry.path(), &dst.join(&entry.file_name()))?;
            } else {
                copy(&entry.path(), &dst.join(&entry.file_name()))?;
            }
        }
    } else {
        copy(src, dst)?;
    }
    Ok(())
}

pub fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    let mut buffer = Vec::new();
    stdin.read_to_end(&mut buffer)?;

    let broken_string: Vec<String> = std::str::from_utf8(&buffer)?
        .split_ascii_whitespace()
        .into_iter()
        .map(|i| i.to_owned())
        .collect();

    Ok(broken_string)
}

// is this path/dir_entry something we should count as a directory for our purposes?
pub fn httm_is_dir<T>(entry: &T) -> bool
where
    T: HttmIsDir,
{
    let path = entry.get_path();
    match entry.get_filetype() {
        Ok(file_type) => match file_type {
            file_type if file_type.is_dir() => true,
            file_type if file_type.is_file() => false,
            file_type if file_type.is_symlink() => {
                match path.read_link() {
                    Ok(link_target) => {
                        // First, read_link() will check symlink is pointing to a directory
                        //
                        // Next, check ancestors() against the read_link() will reduce/remove
                        // infinitely recursive paths, like /usr/bin/X11 pointing to /usr/X11
                        link_target.is_dir()
                            && path.ancestors().all(|ancestor| ancestor != link_target)
                    }
                    // we get an error? still pass the path on, as we get a good path from the dir entry
                    Err(_) => false,
                }
            }
            // char, block, etc devices(?), None/Errs are not dirs, and we have a good path to pass on, so false
            _ => false,
        },
        Err(_) => false,
    }
}

pub trait HttmIsDir {
    fn get_filetype(&self) -> Result<FileType, std::io::Error>;
    fn get_path(&self) -> PathBuf;
}

impl HttmIsDir for Path {
    fn get_filetype(&self) -> Result<FileType, std::io::Error> {
        Ok(self.metadata()?.file_type())
    }
    fn get_path(&self) -> PathBuf {
        self.to_path_buf()
    }
}

impl HttmIsDir for PathData {
    fn get_filetype(&self) -> Result<FileType, std::io::Error> {
        Ok(self.path_buf.metadata()?.file_type())
    }
    fn get_path(&self) -> PathBuf {
        self.path_buf.clone()
    }
}

impl HttmIsDir for DirEntry {
    fn get_filetype(&self) -> Result<FileType, std::io::Error> {
        self.file_type()
    }
    fn get_path(&self) -> PathBuf {
        self.path()
    }
}

impl HttmIsDir for BasicDirEntryInfo {
    fn get_filetype(&self) -> Result<FileType, std::io::Error> {
        //  of course, this is a placeholder error, we just need an error to report back
        //  why not store the error in the struct instead?  because it's more complex.  it might
        //  make it harder to copy around etc
        self.file_type
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn get_path(&self) -> PathBuf {
        self.path.clone()
    }
}

pub fn paint_string<T>(path: T, display_name: &str) -> Cow<str>
where
    T: PaintString,
{
    if path.get_is_phantom() {
        let style = &Style::from_ansi_sequence("38;2;250;200;200;1;0").unwrap_or_default();
        // paint all other phantoms/deleted files the same color, light pink
        let ansi_style = &Style::to_ansi_term_style(style);
        Cow::Owned(ansi_style.paint(display_name).to_string())
    } else if let Some(style) = path.get_ansi_style() {
        let ansi_style = &Style::to_ansi_term_style(&style);
        Cow::Owned(ansi_style.paint(display_name).to_string())
    } else {
        // if a non-phantom file that should not be colored (sometimes -- your regular files)
        // or just in case if all else fails, don't paint and return string
        Cow::Borrowed(display_name)
    }
}

pub trait PaintString {
    fn get_ansi_style(&self) -> Option<lscolors::style::Style>;
    fn get_is_phantom(&self) -> bool;
}

impl PaintString for &PathData {
    fn get_ansi_style(&self) -> Option<lscolors::style::Style> {
        let ls_colors = LsColors::from_env().unwrap_or_default();
        let style = ls_colors.style_for_path(self.path_buf.as_path());
        style.cloned()
    }
    fn get_is_phantom(&self) -> bool {
        self.is_phantom
    }
}

impl PaintString for &SelectionCandidate {
    fn get_ansi_style(&self) -> Option<lscolors::style::Style> {
        let ls_colors = LsColors::from_env().unwrap_or_default();
        let style = ls_colors.style_for(self);
        style.cloned()
    }
    fn get_is_phantom(&self) -> bool {
        self.is_phantom
    }
}

pub fn install_hot_keys() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // get our home directory
    let home_dir = if let Ok(home) = std::env::var("HOME") {
        if let Ok(path) = PathBuf::from(&home).canonicalize() {
            path
        } else {
            return Err(HttmError::new(
                "$HOME, as set in your environment, does not appear to exist",
            )
            .into());
        }
    } else {
        return Err(HttmError::new("$HOME does not appear to be set in your environment").into());
    };

    // check whether httm-key-bindings.zsh is already sourced
    // and, if not, open ~/.zshrc append only for sourcing the httm-key-bindings.zsh
    let mut buffer = String::new();
    let zshrc_path: PathBuf = home_dir.join(".zshrc");
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
            eprintln!("httm: .httm-key-bindings.zsh is already present in user's home directory.");
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

pub fn get_common_path<I, P>(paths: I) -> Option<PathBuf>
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

    if !common_path.as_os_str().is_empty() && common_path.as_os_str() != RootDir.as_os_str() {
        Some(common_path)
    } else {
        None
    }
}

pub fn print_output_buf(
    output_buf: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // mutex keeps threads from writing over each other
    let out = std::io::stdout();
    let mut out_locked = out.lock();
    write!(out_locked, "{}", output_buf)?;
    out_locked.flush()?;

    Ok(())
}

#[derive(Debug)]
pub struct HttmError {
    pub details: String,
}

impl HttmError {
    pub fn new(msg: &str) -> Self {
        HttmError {
            details: msg.to_owned(),
        }
    }
    pub fn with_context(msg: &str, err: Box<dyn Error + 'static>) -> Self {
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

// only the most basic data from a DirEntry
// for use to display in browse window and internally
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct BasicDirEntryInfo {
    pub file_name: OsString,
    pub path: PathBuf,
    pub file_type: Option<FileType>,
}

impl From<&DirEntry> for BasicDirEntryInfo {
    fn from(dir_entry: &DirEntry) -> Self {
        BasicDirEntryInfo {
            file_name: dir_entry.file_name(),
            path: dir_entry.path(),
            file_type: dir_entry.file_type().ok(),
        }
    }
}

// detailed info required to differentiate and display file versions
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PathData {
    pub system_time: SystemTime,
    pub size: u64,
    pub path_buf: PathBuf,
    pub is_phantom: bool,
}

impl cmp::PartialOrd for PathData {
    #[inline]
    fn partial_cmp(&self, other: &PathData) -> Option<cmp::Ordering> {
        Some(self.path_buf.cmp(&other.path_buf))
    }
}

impl cmp::Ord for PathData {
    #[inline]
    fn cmp(&self, other: &PathData) -> cmp::Ordering {
        self.path_buf.cmp(&other.path_buf)
    }
}

impl From<&Path> for PathData {
    fn from(path: &Path) -> Self {
        let metadata_res = symlink_metadata(path).ok();
        PathData::from_parts(path, metadata_res)
    }
}

impl From<&DirEntry> for PathData {
    fn from(dir_entry: &DirEntry) -> Self {
        let metadata_res = dir_entry.metadata().ok();
        let path = dir_entry.path();
        PathData::from_parts(&path, metadata_res)
    }
}

impl PathData {
    fn from_parts(path: &Path, metadata_res: Option<Metadata>) -> Self {
        let absolute_path: PathBuf = if path.is_relative() {
            if let Ok(canonical_path) = path.canonicalize() {
                canonical_path
            } else {
                // canonicalize() on any path that DNE will throw an error
                //
                // in general we handle those cases elsewhere, like the ingest
                // of input files in Config::from for deleted relative paths, etc.
                path.to_path_buf()
            }
        } else {
            path.to_path_buf()
        };

        // call symlink_metadata, as we need to resolve symlinks to get non-"phantom" metadata
        let (len, time, phantom) = match metadata_res {
            Some(md) => {
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
            None => {
                let len = PHANTOM_SIZE;
                let time = PHANTOM_DATE;
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
