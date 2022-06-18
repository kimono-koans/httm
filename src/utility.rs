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

use std::{
    borrow::Cow,
    fs::OpenOptions,
    fs::{copy, create_dir_all, read_dir, DirEntry, FileType},
    io::{self, Read, Write},
    path::{Component::RootDir, Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use lscolors::{LsColors, Style};

use crate::interactive::SelectionCandidate;
use crate::{BasicDirEntryInfo, HttmError, PathData};

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
    let mut ret = path_iter.next()?.as_ref().to_path_buf();

    for path in path_iter {
        if let Some(res) = compare_path_components(ret, path.as_ref()) {
            ret = res;
        } else {
            return None;
        }
    }

    Some(ret)
}

fn compare_path_components<A: AsRef<Path>, B: AsRef<Path>>(a: A, b: B) -> Option<PathBuf> {
    // skip the root dir,
    let a_components = a.as_ref().components();
    let b_components = b.as_ref().components();
    let mut common_path = PathBuf::new();
    let mut has_common_path = false;

    for (a_path, b_path) in a_components.zip(b_components) {
        if a_path == b_path {
            common_path.push(a_path);
            if a_path != RootDir {
                has_common_path = true;
            }
        } else {
            break;
        }
    }

    if has_common_path {
        Some(common_path)
    } else {
        None
    }
}
