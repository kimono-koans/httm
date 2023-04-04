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
use std::path::{Path};
use std::process::Child;
use std::process::Command as ExecProcess;
use std::process::Stdio;

use which::which;

use crate::data::paths::PathData;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::user_has_effective_root;
use crate::GLOBAL_CONFIG;

#[derive(Copy, Clone, PartialEq, Eq)]
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

impl std::cmp::PartialOrd for BasicBlockLocation {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))  
    }
}

pub struct FileDefrag;

impl FileDefrag {
    pub fn exec(path: &Path) -> HttmResult<()> {
        user_has_effective_root()?;

        let pathdata = PathData::from(path);
        let proximate_dataset_mount = pathdata.get_proximate_dataset(&GLOBAL_CONFIG.dataset_collection.map_of_datasets)?;
        let relative_path = pathdata.get_relative_path(proximate_dataset_mount)?;
        let dataset_name = Self::get_dataset_path(proximate_dataset_mount)?;

        let mut process_handle = Self::exec_debug(&dataset_name, relative_path)?;

        let mut stream = Self::ingest(&mut process_handle)?;
        let fragmentation = Self::get_fragmentation(&mut stream);

        eprintln!("Fragmentation Level: {}", fragmentation);

        Ok(())

    }

    fn exec_debug(
        proximate_dataset_name: &str,
        relative_path: &Path,
    ) -> HttmResult<Child> {
        let zfs_command = which("zdb").map_err(|_err| {
            HttmError::new("'zdb' command not found. Make sure the command 'zdb' is in your path.")
        })?;
        let mut process_args = vec!["-vvvvv", "-O"];

        let relpath_str = relative_path.to_string_lossy();

        process_args.push(proximate_dataset_name);
        process_args.push(relpath_str.as_ref());

        let process_handle = ExecProcess::new(zfs_command)
            .args(&process_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(process_handle)
    }

    fn get_dataset_path(
        proximate_dataset_mount: &Path,
    ) -> HttmResult<String> {
        let opt_dataset_info = GLOBAL_CONFIG
            .dataset_collection
            .map_of_datasets
            .get(proximate_dataset_mount);

        match opt_dataset_info {
            Some(info) => Ok(info.source.clone()),
            None => {
                Err(HttmError::new("Could not determine dataset path.").into())
            }
        }
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
                let (lhs, _rhs) = line.split_once("L0 ").unwrap().1.split_once(' ').unwrap();

                let vec: Vec<&str> = lhs.split(':').collect();

                BasicBlockLocation {
                    vdev: u64::from_str_radix(vec.get(0).unwrap(), 16).unwrap(),
                    offset: u128::from_str_radix(vec.get(1).unwrap(), 16).unwrap(),
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

    fn get_fragmentation<I>(stream: I) -> usize
    where
        I: Iterator<Item = BasicBlockLocation>,
    {
        let mut total_num_blocks = 0usize;
        let mut total_num_gaps = 0usize;
        let mut last = None;

        for item in stream {
            total_num_blocks += 1;

            let opt_item = Some(item);

            if last.map(|inner: BasicBlockLocation| inner.vdev) != opt_item.map(|inner| inner.vdev) {
                total_num_gaps += 1; 
            } else if last.map(|inner: BasicBlockLocation| inner.offset + 1 ) != opt_item.map(|inner| inner.offset) {
                total_num_gaps += 1;
            }

            last = opt_item;
        }
            

        total_num_gaps.checked_div(total_num_blocks).unwrap()
    }
        
}
