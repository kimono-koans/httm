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
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
    process::Command as ExecProcess,
};

use fxhash::FxHashMap as HashMap;
use proc_mounts::MountIter;
use rayon::prelude::*;
use which::which;

use crate::HttmError;
use crate::{lookup::get_alt_replicated_dataset, FilesystemInfo, FilesystemType};

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

pub fn get_filesystems_list(
    filesystem_info: &FilesystemInfo,
) -> Result<HashMap<PathBuf, String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let res = if cfg!(target_os = "linux") {
        parse_from_proc_mounts(filesystem_info)?
    } else {
        parse_from_mount_cmd(filesystem_info)?
    };

    Ok(res)
}

// both faster and necessary for certain btrfs features
// allows us to read subvolumes
fn parse_from_proc_mounts(
    filesystem_info: &FilesystemInfo,
) -> Result<HashMap<PathBuf, String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mount_collection: HashMap<PathBuf, String> = MountIter::new()?
        .into_iter()
        .par_bridge()
        .flatten()
        .filter(|mount_info| mount_info.fstype.contains(&filesystem_info.filesystem_name))
        // but exclude snapshot mounts.  we want the raw filesystem names.
        .filter(|mount_info| {
            !mount_info
                .dest
                .to_string_lossy()
                .contains(&filesystem_info.snapshot_dir)
        })
        .map(|mount_info| match filesystem_info.filesystem_type {
            FilesystemType::Zfs => (
                mount_info.dest,
                mount_info.source.to_string_lossy().to_string(),
            ),
            FilesystemType::BtrfsSnapper | FilesystemType::BtrfsTimeshift(_) => {
                let keyed_options: HashMap<String, String> = mount_info
                    .options
                    .into_iter()
                    .filter(|line| line.contains('='))
                    .filter_map(|line| {
                        line.split_once(&"=")
                            .map(|(key, value)| (key.to_owned(), value.to_owned()))
                    })
                    .collect();

                let subvol = match keyed_options.get("subvol") {
                    Some(subvol) => subvol.to_owned(),
                    None => mount_info.source.to_string_lossy().to_string(),
                };
                (mount_info.dest, subvol)
            }
        })
        .filter(|(mount, _dataset)| mount.exists())
        .collect();

    if mount_collection.is_empty() {
        Err(HttmError::new("httm could not find any valid datasets on the system.").into())
    } else {
        Ok(mount_collection)
    }
}

fn parse_from_mount_cmd(
    filesystem_info: &FilesystemInfo,
) -> Result<HashMap<PathBuf, String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // read datasets from 'mount' if possible -- this is much faster than using zfs command
    // but I trust we've parsed it correctly less, because BSD and Linux output are different
    fn get_filesystems_and_mountpoints(
        filesystem_info: &FilesystemInfo,
        shell_command: &PathBuf,
        mount_command: &PathBuf,
    ) -> Result<HashMap<PathBuf, String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let command_output = std::str::from_utf8(
            &ExecProcess::new(&shell_command)
                .arg("-c")
                .arg(mount_command)
                .output()?
                .stdout,
        )?
        .to_owned();

        // parse "mount" for filesystems and mountpoints
        let mount_collection: HashMap<PathBuf, String> = command_output
            .par_lines()
            // want zfs 
            .filter(|line| line.contains(&filesystem_info.filesystem_name))
            // but exclude snapshot mounts.  we want the raw filesystem names.
            .filter(|line| !line.contains(&filesystem_info.snapshot_dir))
            .filter_map(|line|
                // GNU Linux mount output
                if line.contains("type") {
                    line.split_once(&" type")
                // Busybox and BSD mount output
                } else {
                    line.split_once(&" (")
                }
            )
            .map(|(filesystem_and_mount,_)| filesystem_and_mount )
            .filter_map(|filesystem_and_mount| filesystem_and_mount.split_once(&" on "))
            // sanity check: does the filesystem exist? if not, filter it out
            .map(|(filesystem, mount)| (filesystem.to_owned(), PathBuf::from(mount)))
            .filter(|(_filesystem, mount)| mount.exists())
            .map(|(filesystem, mount)| (mount, filesystem))
            .collect();

        if mount_collection.is_empty() {
            Err(HttmError::new("httm could not find any valid datasets on the system.").into())
        } else {
            Ok(mount_collection)
        }
    }

    // do we have the necessary commands for search if user has not defined a snap point?
    // if so run the mount search, if not print some errors
    if let Ok(shell_command) = which("sh") {
        if let Ok(mount_command) = which("mount") {
            get_filesystems_and_mountpoints(filesystem_info, &shell_command, &mount_command)
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

pub fn precompute_alt_replicated(
    mount_collection: &HashMap<PathBuf, String>,
) -> HashMap<PathBuf, Vec<(PathBuf, PathBuf)>> {
    mount_collection
        .par_iter()
        .filter_map(
            |(mount, _dataset)| match get_alt_replicated_dataset(mount, mount_collection) {
                Ok(alt_dataset) => Some((mount.to_owned(), alt_dataset)),
                Err(_err) => None,
            },
        )
        .collect()
}
