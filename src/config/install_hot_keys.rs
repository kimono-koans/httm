//       ___           ___           ___           ___
//      /\__\         /\  \         /\  \         /\__\
//     /:/  /         \:\  \        \:\  \       /::|  |
//    /:/__/           \:\  \        \:\  \     /:|:|  |
//   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
//  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
//  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
//       \::/  /    /:/  /        /:/  /            /:/  /
//    s   /:/  /     \/__/         \/__/            /:/  /
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
};

use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::make_tmp_path;

const HTTM_SCRIPT_PATH: &str = ".httm-key-bindings.zsh";
const ZSHRC_PATH: &str = ".zshrc";

pub fn install_hot_keys() -> HttmResult<()> {
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
    let zshrc_path: PathBuf = home_dir.join(ZSHRC_PATH);
    let mut zshrc_file = OpenOptions::new()
        .read(true)
        .write(true)
        .append(true)
        .open(zshrc_path)
        .map_err(|err| {
            HttmError::with_context(
                "Opening user's ~/.zshrc file failed for the following reason: ",
                err.into(),
            )
        })?;

    // read current zshrc to string buffer
    zshrc_file.read_to_string(&mut buffer)?;

    // check that there are not lines in the zshrc that contain "source" and "httm-key-bindings.zsh"
    if !buffer
        .lines()
        .filter(|line| !line.starts_with('#'))
        .any(|line| line.contains("source") && line.contains("httm-key-bindings.zsh"))
    {
        // append "source ~/.httm-key-bindings.zsh" to zshrc
        zshrc_file.write_all(
            "\n# httm: zsh hot keys script\nsource ~/.httm-key-bindings.zsh\n".as_bytes(),
        )?;
    } else {
        return Err(HttmError::new(
            "httm: zsh hot keys appear to already be sourced in the user's ~/.zshrc. Quitting. ",
        )
        .into());
    }

    // create key binding file -- done at compile time
    let zsh_hot_key_script = include_str!("../../scripts/httm-key-bindings.zsh");

    // create paths to use
    let zsh_script_path: PathBuf = [&home_dir, &PathBuf::from(HTTM_SCRIPT_PATH)]
        .iter()
        .collect();
    let zsh_script_tmp_path = make_tmp_path(zsh_script_path.as_path());

    // create tmp file in user's home dir or will fail if file already exists
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&zsh_script_tmp_path)
    {
        Ok(mut zsh_script_file) => {
            // write the byte string
            zsh_script_file.write_all(zsh_hot_key_script.as_bytes())?;

            // close the file
            drop(zsh_script_file);

            // then move tmp file to the final location
            match std::fs::rename(
                zsh_script_tmp_path,
                zsh_script_path,
            ) {
                Ok(_) => {
                    eprintln!("httm: zsh hot keys were installed successfully.");
                    std::process::exit(0)
                }
                Err(err) => {
                    Err(HttmError::with_context("httm: could not move .httm-key-bindings.zsh.tmp to .httm-key-bindings.zsh for the following reason: ", err.into()).into())
                }
            }
        }
        Err(err) => Err(HttmError::with_context(
            "Opening ~/.httm-key-bindings.zsh.tmp file failed for the following reason: ",
            err.into(),
        )
        .into()),
    }
}
