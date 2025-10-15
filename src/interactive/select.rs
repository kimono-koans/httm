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
use crate::config::generate::{PrintMode, SelectMode};
use crate::interactive::preview::PreviewSelection;
use crate::interactive::view_mode::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{delimiter, print_output_buf};
use crate::lookup::versions::VersionsMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;

#[allow(dead_code)]
pub struct InteractiveSelect {
    view_mode: ViewMode,
    snap_path_strings: Vec<String>,
    opt_live_version: Option<String>,
}

impl InteractiveSelect {
    pub fn new(
        view_mode: ViewMode,
        snap_path_strings: Vec<String>,
        opt_live_version: Option<String>,
    ) -> Self {
        Self {
            view_mode,
            snap_path_strings,
            opt_live_version,
        }
    }

    pub fn last_snap(map: &VersionsMap) -> Vec<String> {
        map.iter()
            .filter_map(|(key, values)| {
                if values.is_empty() {
                    eprintln!(
                        "WARN: No last snap of {:?} is available for selection.  Perhaps you omitted identical files.",
                        key.path()
                    );
                    None
                } else {
                    Some(values)
                }
            })
            .flatten()
            .map(|path_data| path_data.path().to_string_lossy().to_string())
            .collect()
    }

    pub fn print_selections(&self, select_mode: &SelectMode) -> HttmResult<()> {
        self.snap_path_strings
            .iter()
            .map(Path::new)
            .try_for_each(|snap_path| self.print_snap_path(snap_path, select_mode))
    }

    fn print_snap_path(&self, snap_path: &Path, select_mode: &SelectMode) -> HttmResult<()> {
        match select_mode {
            SelectMode::Path => {
                let delimiter = delimiter();
                let output_buf = match GLOBAL_CONFIG.print_mode {
                    PrintMode::Raw(_) => {
                        format!("{}{delimiter}", snap_path.to_string_lossy())
                    }
                    PrintMode::Formatted(_) => {
                        format!("\"{}\"{delimiter}", snap_path.to_string_lossy())
                    }
                };

                print_output_buf(&output_buf)
            }
            SelectMode::Contents => {
                if !snap_path.is_file() {
                    let description = format!("Path is not a file: {:?}", snap_path);
                    return HttmError::from(description).into();
                }
                let mut f = std::fs::OpenOptions::new().read(true).open(snap_path)?;
                let mut contents = Vec::new();
                f.read_to_end(&mut contents)?;

                // SAFETY: Panic here is not the end of the world as we are just printing the bytes.
                // This is the same as simply `cat`-ing the file.
                let output_buf = unsafe { std::str::from_utf8_unchecked(&contents) };

                print_output_buf(output_buf)
            }
            SelectMode::Preview => {
                let view_mode = &self.view_mode;

                let preview_selection = PreviewSelection::new(&view_mode)?;

                let cmd = if let Some(command) = preview_selection.opt_preview_command() {
                    command.replace("$snap_file", &format!("{:?}", snap_path))
                } else {
                    return HttmError::new("Could not parse preview command").into();
                };

                let env_command =
                    which::which("env").unwrap_or_else(|_| PathBuf::from("/usr/bin/env"));

                let spawned = ExecProcess::new(env_command)
                    .arg("bash")
                    .arg("-c")
                    .arg(cmd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?;

                match spawned.stdout {
                    Some(mut stdout) => {
                        let mut output_buf = String::new();
                        stdout.read_to_string(&mut output_buf)?;
                        print_output_buf(&output_buf)
                    }
                    None => match spawned.stderr {
                        Some(mut stderr) => {
                            let mut output_buf = String::new();
                            stderr.read_to_string(&mut output_buf)?;
                            if !output_buf.is_empty() {
                                eprintln!("{}", &output_buf)
                            }
                            Ok(())
                        }
                        None => {
                            let description = format!(
                                "Preview command output was empty for path: {:?}",
                                snap_path
                            );
                            HttmError::from(description).into()
                        }
                    },
                }
            }
        }
    }
}
