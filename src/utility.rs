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

use chrono::{DateTime, Local};
use lscolors::{LsColors, Style};
use std::{
    fs::{copy, create_dir_all, read_dir, DirEntry, FileType},
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub fn timestamp_file(system_time: &SystemTime) -> String {
    let date_time: DateTime<Local> = system_time.to_owned().into();
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

pub fn paint_string(path: &Path, file_name: &str) -> String {
    let ls_colors = LsColors::from_env().unwrap_or_default();

    if let Some(style) = ls_colors.style_for_path(path) {
        let ansi_style = &Style::to_ansi_term_style(style);
        ansi_style.paint(file_name).to_string()
    } else {
        file_name.to_owned()
    }
}

pub fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut buffer = Vec::new();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
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
                    Ok(link) => {
                        // First, read_link() will check symlink is pointing to a directory
                        //
                        // Next, check ancestors() against the read_link() will reduce/remove
                        // infinitely recursive paths, like /usr/bin/X11 pointing to /usr/X11
                        link.is_dir() && link.ancestors().all(|ancestor| ancestor != link)
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

impl HttmIsDir for PathBuf {
    fn get_filetype(&self) -> Result<FileType, std::io::Error> {
        Ok(self.metadata()?.file_type())
    }
    fn get_path(&self) -> PathBuf {
        self.to_path_buf()
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
