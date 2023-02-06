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

use std::path::PathBuf;

use which::which;

use crate::config::generate::Config;
use crate::exec::interactive::ViewMode;
use crate::library::results::{HttmError, HttmResult};

pub struct PreviewSelection {
    pub opt_preview_window: Option<String>,
    pub opt_preview_command: Option<String>,
}

impl PreviewSelection {
    pub fn new(config: &Config, view_mode: ViewMode) -> HttmResult<Self> {
        //let (opt_preview_window, opt_preview_command) =
        let res = match &config.opt_preview {
            Some(defined_command) if matches!(view_mode, ViewMode::Select(_)) => {
                let opt_live_version = if let ViewMode::Select(opt) = view_mode {
                    opt
                } else {
                    unreachable!()
                };

                PreviewSelection {
                    opt_preview_window: Some("up:50%".to_owned()),
                    opt_preview_command: Some(Self::parse_preview_command(
                        defined_command,
                        &opt_live_version,
                    )?),
                }
            }
            _ => PreviewSelection {
                opt_preview_window: Some(String::new()),
                opt_preview_command: None,
            },
        };

        Ok(res)
    }

    fn parse_preview_command(
        defined_command: &str,
        opt_live_version: &Option<String>,
    ) -> HttmResult<String> {
        let command = if defined_command == "default" {
            match opt_live_version {
                Some(live_version) if PathBuf::from(live_version).exists() && which("bowie").is_ok() => {
                    format!("bowie --direct \"$snap_file\" \"{live_version}\"")
                },
                _ => match which("cat") {
                    Ok(_) => "cat \"$snap_file\"".to_string(),
                    Err(_) => {
                        return Err(HttmError::new(
                            "'cat' executable could not be found in the user's PATH. 'cat' is necessary for executing a bare preview command.",
                        )
                        .into())
                    }
                },
            }
        } else {
            match defined_command.split_ascii_whitespace().next() {
                Some(potential_executable) => {
                    if which(potential_executable).is_err() {
                        return Err(HttmError::new("User specified a preview variable for a live version, but a live version for the file selected does not exist.").into());
                    }
                }
                None => {
                    return Err(HttmError::new(
                        "httm could not determine a valid preview command from user's input.",
                    )
                    .into());
                }
            }

            let parsed_command = match opt_live_version {
                Some(live_version) if defined_command.contains("{live_file}") && !PathBuf::from(live_version).exists() => {
                    return Err(HttmError::new("User specified a preview variable for a live version, but a live version for the file selected does not exist.").into())
                },
                Some(live_version) => {
                    defined_command
                        .replace("{snap_file}", "\"$snap_file\"")
                        .replace("{live_file}", format!("\"{live_version}\"").as_str())
                },
                None if defined_command.contains("{live_file}") => {
                    return Err(HttmError::new("User specified a preview variable for a live version, but a live version could not be determined.").into())
                },
                None => {
                    defined_command
                        .replace("{snap_file}", "\"$snap_file\"")
                },
            };

            // protect ourselves from command like cat
            // just waiting on stdin by appending the snap file
            if !parsed_command.contains("\"$snap_file\"") {
                [defined_command, " \"$snap_file\""].into_iter().collect()
            } else {
                parsed_command
            }
        };

        let res = match which("cut") {
            Ok(_) => {
                format!(
                    "snap_file=\"`echo {{}} | cut -d'\"' -f2`\"; if test -f \"$snap_file\" || test -d \"$snap_file\" || test -L \"$snap_file\"; then exec 0<&-; {command} 2>&1; fi"
                )
            }
            Err(_) => {
                return Err(
                    HttmError::new("'cut' executable could not be found in the user's PATH. 'cut' is necessary for executing a preview command.").into(),
                )
            }
        };

        Ok(res)
    }
}
