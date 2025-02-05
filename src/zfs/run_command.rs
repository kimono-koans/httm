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

use crate::data::paths::{PathData, PathDeconstruction};
use crate::filesystem::mounts::FilesystemType;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::user_has_effective_root;
use crate::roll_forward::exec::RollForward;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ExecProcess, Stdio};
use which::which;

pub struct RunZFSCommand {
    zfs_command: PathBuf,
}

impl RunZFSCommand {
    pub fn new() -> HttmResult<Self> {
        let zfs_command = which("zfs").map_err(|_err| {
            HttmError::new("'zfs' command not found. Make sure the command 'zfs' is in your path.")
        })?;

        Ok(Self { zfs_command })
    }

    pub fn version(&self) -> HttmResult<String> {
        let process_output = ExecProcess::new(&self.zfs_command).arg("-V").output()?;

        if !process_output.stderr.is_empty() {
            return Err(HttmError::new(std::str::from_utf8(&process_output.stderr)?).into());
        }

        Ok(std::string::String::from_utf8(process_output.stdout)?)
    }

    pub fn snapshot(&self, snapshot_names: &[String]) -> HttmResult<()> {
        let mut process_args = vec!["snapshot".to_owned()];

        process_args.extend_from_slice(snapshot_names);

        let process_output = ExecProcess::new(&self.zfs_command)
            .args(&process_args)
            .output()?;

        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot create snapshots : permission denied") {
                "httm must have root privileges to snapshot a filesystem".to_owned()
            } else {
                "httm was unable to take snapshots. The 'zfs' command issued the following error: "
                    .to_owned()
                    + stderr_string
            };

            return Err(HttmError::new(&msg).into());
        }

        Ok(())
    }

    pub fn rollback(&self, snapshot_names: &[String]) -> HttmResult<()> {
        let mut process_args = vec!["rollback".to_owned(), "-r".to_owned()];

        process_args.extend_from_slice(snapshot_names);

        let process_output = ExecProcess::new(&self.zfs_command)
            .args(&process_args)
            .output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                "httm may need root privileges to 'zfs rollback' a filesystem".to_owned()
            } else {
                "httm was unable to rollback the snapshot name. The 'zfs' command issued the following error: ".to_owned() + stderr_string
            };

            return Err(HttmError::new(&msg).into());
        }

        Ok(())
    }

    pub fn prune(&self, snapshot_names: &[String]) -> HttmResult<()> {
        let mut process_args = vec!["destroy".to_owned(), "-r".to_owned()];

        process_args.extend_from_slice(snapshot_names);

        let process_output = ExecProcess::new(&self.zfs_command)
            .args(&process_args)
            .output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = if stderr_string.contains("cannot destroy snapshots: permission denied") {
                "httm must have root privileges to destroy a snapshot filesystem".to_owned()
            } else {
                "httm was unable to destroy snapshots. The 'zfs' command issued the following error: "
                .to_owned()
                + stderr_string
            };

            return Err(HttmError::new(&msg).into());
        }

        Ok(())
    }

    pub fn allow(&self, fs_name: &str, allow_type: &ZfsAllowPriv) -> HttmResult<()> {
        let process_args = vec!["allow", fs_name];

        let process_output = ExecProcess::new(&self.zfs_command)
            .args(&process_args)
            .output()?;
        let stderr_string = std::str::from_utf8(&process_output.stderr)?.trim();
        let stdout_string: &str = std::str::from_utf8(&process_output.stdout)?.trim();

        // stderr_string is a string not an error, so here we build an err or output
        if !stderr_string.is_empty() {
            let msg = "httm was unable to determine 'zfs allow' for the path given. The 'zfs' command issued the following error: ".to_owned() + stderr_string;

            return Err(HttmError::new(&msg).into());
        }

        let user_name = std::env::var("USER")?;

        if !stdout_string.contains(&user_name)
            || !allow_type
                .as_zfs_cmd_strings()
                .iter()
                .all(|p| stdout_string.contains(p))
        {
            let msg = "User does not have 'zfs allow' privileges for the path given.";

            return Err(HttmError::new(msg).into());
        }

        Ok(())
    }

    pub fn diff(&self, roll_forward: &RollForward) -> HttmResult<Child> {
        // -H: tab separated, -t: Specify time, -h: Normalize paths (don't use escape codes)
        let full_name = roll_forward.full_name();
        let process_args = vec!["diff", "-H", "-t", "-h", &full_name];

        let process_handle = ExecProcess::new(&self.zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(process_handle)
    }
}

pub enum ZfsAllowPriv {
    Snapshot,
    Rollback,
}

impl ZfsAllowPriv {
    pub fn from_path(&self, path: &Path) -> HttmResult<PathBuf> {
        let path_data = PathData::from(path);

        ZfsAllowPriv::from_opt_proximate_dataset(&self, &path_data, None)
    }

    pub fn from_opt_proximate_dataset(
        &self,
        path_data: &PathData,
        opt_proximate_dataset: Option<&Path>,
    ) -> HttmResult<PathBuf> {
        let Some(fs_name) = path_data.source(opt_proximate_dataset) else {
            let msg = format!(
                "Could not determine dataset name from path given: {:?}",
                path_data.path()
            );
            return Err(HttmError::new(&msg).into());
        };

        match path_data.fs_type(opt_proximate_dataset) {
            Some(FilesystemType::Zfs) => {}
            _ => {
                let msg = format!(
                    "httm only supports snapshot guards for ZFS paths.  Path is not located on a ZFS dataset: {:?}",
                    path_data.path()
                );
                return Err(HttmError::new(&msg).into());
            }
        }

        Self::from_fs_name(&self, &fs_name.to_string_lossy())?;

        Ok(fs_name)
    }

    pub fn from_fs_name(&self, fs_name: &str) -> HttmResult<()> {
        let msg = match self {
            ZfsAllowPriv::Rollback => "A rollback after a restore action",
            ZfsAllowPriv::Snapshot => "A snapshot guard before restore action",
        };

        if let Err(root_error) = user_has_effective_root(msg) {
            if let Err(_allow_priv_error) = self.user_has_zfs_allow_priv(fs_name) {
                return Err(root_error);
            }
        }

        Ok(())
    }

    fn as_zfs_cmd_strings(&self) -> &[&str] {
        match self {
            ZfsAllowPriv::Rollback => &["rollback"],
            ZfsAllowPriv::Snapshot => &["snapshot", "mount"],
        }
    }

    fn user_has_zfs_allow_priv(&self, fs_name: &str) -> HttmResult<()> {
        let run_zfs = RunZFSCommand::new()?;
        run_zfs.allow(fs_name, self)
    }
}
