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

use crate::config::generate::{PrintMode, SelectMode};
use crate::display_versions::wrapper::VersionsDisplayWrapper;
use crate::interactive::preview::PreviewSelection;
use crate::interactive::view_mode::MultiSelect;
use crate::interactive::view_mode::ViewMode;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{delimiter, print_output_buf};
use crate::lookup::versions::VersionsMap;
use crate::Config;
use crate::GLOBAL_CONFIG;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;

use super::browse::InteractiveBrowse;

pub struct InteractiveSelect {
    pub view_mode: ViewMode,
    pub snap_path_strings: Vec<String>,
    pub opt_live_version: Option<String>,
}

impl TryFrom<&mut InteractiveBrowse> for InteractiveSelect {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(interactive_browse: &mut InteractiveBrowse) -> HttmResult<Self> {
        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &interactive_browse.selected_pathdata)?;

        // snap and live set has no snaps
        if versions_map.is_empty() {
            let paths: Vec<String> = interactive_browse
                .selected_pathdata
                .iter()
                .map(|path| path.path_buf.to_string_lossy().to_string())
                .collect();
            let msg = format!(
                "{}{:?}",
                "Cannot select or restore from the following paths as they have no snapshots:\n",
                paths
            );
            return Err(HttmError::new(&msg).into());
        }

        let opt_live_version: Option<String> = if interactive_browse.selected_pathdata.len() > 1 {
            None
        } else {
            interactive_browse
                .selected_pathdata
                .get(0)
                .map(|pathdata| pathdata.path_buf.to_string_lossy().into_owned())
        };

        let view_mode = ViewMode::Select(opt_live_version.clone());

        let snap_path_strings = if GLOBAL_CONFIG.opt_last_snap.is_some() {
            Self::last_snap(&versions_map)
        } else {
            // same stuff we do at fn exec, snooze...
            let display_config = Config::from(interactive_browse.selected_pathdata.clone());

            let display_map = VersionsDisplayWrapper::from(&display_config, versions_map);

            let selection_buffer = display_map.to_string();

            display_map.map.iter().try_for_each(|(live, snaps)| {
                if snaps.is_empty() {
                    let msg = format!("WARN: Path {:?} has no snapshots available.", live.path_buf);
                    return Err(HttmError::new(&msg));
                }

                Ok(())
            })?;

            // loop until user selects a valid snapshot version
            loop {
                // get the file name
                let selected_line = view_mode.view_buffer(&selection_buffer, MultiSelect::On)?;

                if let Some(background_handle) = interactive_browse.opt_background_handle.take() {
                    let _ = background_handle.join();
                }

                let requested_file_names = selected_line
                    .iter()
                    .filter_map(|selection| {
                        // ... we want everything between the quotes
                        selection
                            .split_once("\"")
                            .and_then(|(_lhs, rhs)| rhs.rsplit_once("\""))
                            .map(|(lhs, _rhs)| lhs)
                    })
                    .filter(|selection_buffer| {
                        // and cannot select a 'live' version or other invalid value.
                        display_map
                            .keys()
                            .all(|key| key.path_buf.as_path() != Path::new(selection_buffer))
                    })
                    .map(|selection_buffer| selection_buffer.to_string())
                    .collect::<Vec<String>>();

                if requested_file_names.is_empty() {
                    continue;
                }

                break requested_file_names;
            }
        };

        Ok(Self {
            view_mode,
            snap_path_strings,
            opt_live_version,
        })
    }
}

impl InteractiveSelect {
    fn last_snap(map: &VersionsMap) -> Vec<String> {
        map.iter()
            .filter_map(|(key, values)| {
                if values.is_empty() {
                    eprintln!(
                        "WARN: No last snap of {:?} is available for selection.  Perhaps you omitted identical files.",
                        key.path_buf
                    );
                    None
                } else {
                    Some(values)
                }
            })
            .flatten()
            .map(|pathdata| pathdata.path_buf.to_string_lossy().to_string())
            .collect()
    }

    pub fn print_selections(&self, select_mode: &SelectMode) -> HttmResult<()> {
        self.snap_path_strings
            .iter()
            .map(Path::new)
            .try_for_each(|snap_path| self.print_snap_path(snap_path, select_mode))?;

        Ok(())
    }

    fn print_snap_path(&self, snap_path: &Path, select_mode: &SelectMode) -> HttmResult<()> {
        match select_mode {
            SelectMode::Path => {
                let delimiter = delimiter();
                let output_buf = match GLOBAL_CONFIG.print_mode {
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        format!("{}{delimiter}", snap_path.to_string_lossy())
                    }
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        format!("\"{}\"{delimiter}", snap_path.to_string_lossy())
                    }
                };

                print_output_buf(&output_buf)?;

                Ok(())
            }
            SelectMode::Contents => {
                if !snap_path.is_file() {
                    let msg = format!("Path is not a file: {:?}", snap_path);
                    return Err(HttmError::new(&msg).into());
                }
                let mut f = std::fs::File::open(snap_path)?;
                let mut contents = Vec::new();
                f.read_to_end(&mut contents)?;

                // SAFETY: Panic here is not the end of the world as we are just printing the bytes.
                // This is the same as simply `cat`-ing the file.
                let output_buf = unsafe { std::str::from_utf8_unchecked(&contents) };

                print_output_buf(output_buf)?;

                Ok(())
            }
            SelectMode::Preview => {
                let view_mode = &self.view_mode;

                let preview_selection = PreviewSelection::new(&view_mode)?;

                let cmd = if let Some(command) = preview_selection.opt_preview_command {
                    command.replace("$snap_file", &format!("{:?}", snap_path))
                } else {
                    return Err(HttmError::new("Could not parse preview command").into());
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
                            let msg = format!(
                                "Preview command output was empty for path: {:?}",
                                snap_path
                            );
                            Err(HttmError::new(&msg).into())
                        }
                    },
                }
            }
        }
    }
}
