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

use std::cmp::Ordering;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::Command as ExecProcess;
use std::process::Stdio;

use ansi_term::Colour::{Blue, Red};
use once_cell::sync::OnceCell;
use which::which;

use crate::data::paths::PathData;
use crate::library::iter_extensions::HttmIter;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::{AdditionalSnapInfo, PrecautionarySnapType, SnapGuard};
use crate::library::utility::{copy_direct, remove_recursive};
use crate::library::utility::{is_metadata_different, user_has_effective_root};
use crate::GLOBAL_CONFIG;

#[derive(Clone)]
struct BasicBlockLocation {
    vdev: u64,
    offset: u128,
}


impl std::cmp::Ord for BasicBlockLocation {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let initial_ordering = self.vdev.cmp(&other.vdev);

        if initial_ordering.is_eq() {
            return self.offset.cmp(&other.offset);
        }

        initial_ordering
    }
}

pub struct FileDefrag;

impl FileDefrag {
    pub fn exec(full_snap_name: &str) -> HttmResult<()> {
        user_has_effective_root()?;

        let pathdata = PathData::from(path);
        let proximate_dataset_mount = pathdata.get_proximate_dataset(GLOBAL_CONFIG.dataset_collection.map_of_datasets)?;
        let relative_path = pathdata.get_relative_path(proximate_dataset_mount);
        let dataset_name = Self::get_dataset_path(proximate_dataset_mount);

        let mut process_handle = Self::exec_diff(proximate_dataset_name, relative_path)?;

        let mut stream = Self::ingest(&mut process_handle)?;

        let pre_exec_snap_name = SnapGuard::snapshot(
            dataset_name,
            &AdditionalSnapInfo::RollForwardSnapName(snap_name.to_owned()),
            PrecautionarySnapType::PreRollForward,
        )?;

        match Self::roll_forward(&mut stream, snap_name) {
            Ok(_) => {
                println!("httm roll forward completed successfully.");
            }
            Err(err) => {
                let msg = format!(
                    "httm roll forward failed for the following reason: {}.\n\
                Attempting roll back to precautionary pre-execution snapshot.",
                    err
                );
                eprintln!("{}", msg);

                SnapGuard::rollback(&pre_exec_snap_name)
                    .map(|_| println!("Rollback succeeded."))?;

                std::process::exit(1)
            }
        };

        SnapGuard::snapshot(
            dataset_name,
            &AdditionalSnapInfo::RollForwardSnapName(snap_name.to_owned()),
            PrecautionarySnapType::PostRollForward,
        )
        .map(|_res| ())
    }

    fn exec_debug(
        proximate_dataset_name: &str,
        relative_path: &Path,
    ) -> HttmResult<Child> {
        let zfs_command = which("zdb").map_err(|_err| {
            HttmError::new("'zdb' command not found. Make sure the command 'zdb' is in your path.")
        })?;
        let mut process_args = vec!["-vvvvv", "-O"];

        process_args.push(proximate_dataset_name);
        process_args.push(relative_path.to_string_lossy());

        let process_handle = ExecProcess::new(zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(process_handle)
    }

    fn get_dataset_path(
        pathdata: &PathData,
        proximate_dataset_mount: &Path,
    ) -> Option<PathBuf> {
        let opt_dataset_info = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(proximate_dataset_mount);

        opt_dataset_info.map(|info| info.source)
    }

    fn ingest(process_handle: &mut Child) -> HttmResult<impl Iterator<Item = BasicBlockLocation> + '_> {
        let stdout_buffer = if let Some(output) = process_handle.stdout.take() {
            std::io::BufReader::new(output)
        } else {
            println!("'zdb' could not determine the object specified from input");
            std::process::exit(0);
        };

        let res = stdout_buffer
            .lines()
            .map(|line| line.expect("Could not obtain line from string."))
            .filter(|line| line.contains("L0 "))
            .map(move |line| {
                let (lhs, _rhs) = line.split("L0 ").unwrap().split(' ').unwrap();

                let vec: Vec<&str> = lhs.split(':');

                BasicBlockLocation {
                    vdev: u64::from_str_radix(vec.get(0).unwrap()).unwrap(),
                    offset: u128::from_str_radix(vec.get(1).unwrap()).unwrap(),
                }
            });

        if process_handle.stderr.is_some() {
            let mut stderr_buffer = std::io::BufReader::new(process_handle.stderr.take().unwrap());

            let buffer = stderr_buffer.fill_buf()?;

            if !buffer.is_empty() {
                if let Ok(output_buf) = std::str::from_utf8(buffer) {
                    return Err(HttmError::new(output_buf.to_string().trim()).into());
                }
            }
        }

        Ok(res)
    }

    fn print_fragmentation<I>(stream: I) -> HttmResult<()>
    where
        I: Iterator<Item = DiffEvent>,
    {
        let mut vec = stream.into_iter().collect();
        vec.sort();

        
    }
        
}
