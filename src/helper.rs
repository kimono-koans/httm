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
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::Command as ExecProcess,
};
use which::which;

pub fn read_stdin() -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    let mut stdin = stdin.lock();
    stdin.read_line(&mut buffer)?;

    let broken_string: Vec<String> = buffer
        .split_ascii_whitespace()
        .into_iter()
        .map(|i| i.to_owned())
        .collect();

    Ok(broken_string)
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
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build zfs query to execute - in case the fast paths fail
    // this is very slow but we are sure it works everywhere with zfs
    // because the zfs command tab sanely delimits its output
    let meh_performance = |shell_command: &PathBuf,
                           zfs_command: &PathBuf|
     -> Result<
        Vec<std::string::String>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        let command_output = std::str::from_utf8(
            &ExecProcess::new(&shell_command)
                .arg("-c")
                .arg(zfs_command)
                .arg("list -H -t filesystem -o mountpoint,mounted")
                .output()?
                .stdout,
        )?
        .to_owned();

        let res = command_output
            .par_lines()
            .filter(|line| line.contains("yes"))
            .filter_map(|line| line.split('\t').next())
            // sanity check: does the filesystem exist? if not, filter it
            .filter(|line| Path::new(line).exists())
            .map(|line| line.to_owned())
            .collect::<Vec<String>>();
        Ok(res)
    };

    // read datasets from 'mount' if possible -- this is much faster than using zfs command
    // but I trust we've parsed it correctly less, because BSD and Linux output are different
    let good_performance = |shell_command: &PathBuf,
                            mount_command: &PathBuf|
     -> Result<
        Vec<std::string::String>,
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

        // parse "mount -t zfs" for filesystems
        let res: Vec<String> = command_output
            .par_lines()
            .filter(|line| line.contains("zfs"))
            .map(|line| line.split_once(&"on "))
            .flatten()
            .map(|(_,last)| last)
            .map(|line|
                // GNU Linux mount output
                if line.contains("type") {
                    line.split_once(&" type")
                // Busybox and BSD mount output
                } else {
                    line.split_once(&" (")
                })
            .flatten()
            .map(|(first,_)| first)
            // sanity check: does the filesystem exist? if not, filter it
            .filter(|line| Path::new(line).exists())
            .map(|line| line.to_owned())
            .collect();

        Ok(res)
    };

    // read /proc/mounts -- fastest but only works on Linux, least certain the parsing is correct
    // as Linux dumps escaped characters into filesystem strings, and space delimits
    let best_performance = || -> Result<Vec<std::string::String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let mut file = OpenOptions::new()
            .read(true)
            .open(Path::new("/proc/mounts"))?;
        let mut buffer = String::new();
        let _ = &file.read_to_string(&mut buffer)?;

        let res = buffer
            .par_lines()
            .filter(|line| line.contains("zfs"))
            .filter_map(|line| line.split(' ').nth(1))
            .map(|line| line.replace(r#"\040"#, " "))
            // sanity check: does the filesystem exist? if not, filter it
            .filter(|line| Path::new(line).exists())
            .collect::<Vec<String>>();

        Ok(res)
    };

    // not the end of the world if we can't use the fast path.  only really useful for quick non-interactive runs
    // as we store the values in the Config.  Therefore, we want to fail and fallback if there is some sign that parsed
    // input is not current
    if cfg!(target_os = "linux") {
        if let Ok(best) = best_performance() {
            if !best.is_empty() {
                return Ok(best);
            }
        }
    }

    // do we have the necessary commands for search if user has not defined a snap point?
    if let Ok(shell_command) = which("sh") {
        if let Ok(mount_command) = which("mount") {
            if let Ok(good) = good_performance(&shell_command, &mount_command) {
                if !good.is_empty() {
                    return Ok(good);
                }
            }
        } else {
            return Err(HttmError::new(
                "mount command not found. Make sure the command 'mount' is in your path.",
            )
            .into());
        }

        if let Ok(zfs_command) = which("zfs") {
            if let Ok(meh) = meh_performance(&shell_command, &zfs_command) {
                if !meh.is_empty() {
                    return Ok(meh);
                }
            }
        }
    } else {
        return Err(HttmError::new(
            "sh command not found. Make sure the command 'sh' is in your path.",
        )
        .into());
    }
    Err(HttmError::new("httm could not find any valid ZFS datasets on the system.").into())
}
