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

use crate::HttmError;

use rayon::prelude::*;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command as ExecProcess,
};
use which::which;

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
    let zshrc_path: PathBuf = [&home_dir, &PathBuf::from(".zshrc")].iter().collect();
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

pub fn list_all_filesystems(
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // read datasets from 'mount' if possible -- this is much faster than using zfs command
    // but I trust we've parsed it correctly less, because BSD and Linux output are different
    let get_filesystems_and_mountpoints = |shell_command: &PathBuf,
                                           mount_command: &PathBuf|
     -> Result<
        Vec<(String, String)>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let command_output = std::str::from_utf8(
            &ExecProcess::new(&shell_command)
                .arg("-c")
                .arg(mount_command)
                .arg("-t zfs")
                .output()?
                .stdout,
        )?
        .to_owned();

        // parse "mount -t zfs" for filesystems and mountpoints
        let (first, the_rest): (Vec<&str>, Vec<&str>) = command_output
            .par_lines()
            .filter(|line| line.contains("zfs"))
            .filter_map(|line| line.split_once(&" on "))
            .collect();

        let filesystems: Vec<String> = first.into_par_iter().map(|str| str.to_owned()).collect();

        let mount_points: Vec<String> = the_rest
            .into_par_iter()
            .filter_map(|the_rest|
                // GNU Linux mount output
                if the_rest.contains("type") {
                    the_rest.split_once(&" type")
                // Busybox and BSD mount output
                } else {
                    the_rest.split_once(&" (")
                }
            )
            .map(|(first, _)| first)
            .map(|line| line.to_owned())
            .collect();

        if filesystems.is_empty() || mount_points.is_empty() {
            return Err(HttmError::new(
                "httm could not find any valid ZFS datasets on the system.",
            )
            .into());
        }

        let mount_collection: Vec<(String, String)> = filesystems
            .iter()
            .cloned()
            .zip(mount_points.iter().cloned())
            // sanity check: does the filesystem exist? if not, filter it out
            .filter(|(_fs, mount)| Path::new(mount).exists())
            .collect();

        Ok(mount_collection)
    };

    // do we have the necessary commands for search if user has not defined a snap point?
    if let Ok(shell_command) = which("sh") {
        if let Ok(mount_command) = which("mount") {
            get_filesystems_and_mountpoints(&shell_command, &mount_command)
        } else {
            Err(HttmError::new(
                "mount command not found. Make sure the command 'mount' is in your path.",
            )
            .into())
        }
    } else {
        Err(
            HttmError::new("sh command not found. Make sure the command 'sh' is in your path.")
                .into(),
        )
    }
}
