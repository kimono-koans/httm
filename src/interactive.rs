use crate::Config;
use crate::HttmError;

extern crate skim;
use skim::prelude::*;
use std::io::Cursor;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command as ExecProcess,
};

fn interactive_lookup(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // build our paths for the httm preview invocations, we need Strings because for now
    // skim does not allow invoking preview from a Rust function, we actually just exec httm again
    let relative = if let Some(relative_dir) = &config.opt_relative_dir {
        relative_dir.to_string_lossy()
    } else {
        config.working_dir.to_string_lossy()
    };

    let mnt_point = if let Some(mnt_point) = &config.opt_man_mnt_point {
        mnt_point.to_string_lossy()
    } else {
        config.working_dir.to_string_lossy()
    };

    let cp = if let Ok(cp) = requested_dir.canonicalize() {
        cp
    } else {
        config.working_dir.clone()
    };

    // string to exec on each preview
    let preview_str = &format!(
        "httm --mnt-point \"{mnt_point}\" --relative \"{relative}\" \"{}\"/{{}}",
        cp.to_string_lossy()
    );

    let options = SkimOptionsBuilder::default()
        .interactive(true)
        .height(Some("100%"))
        .preview_window(Some("70%"))
        .multi(true)
        .preview(Some(preview_str))
        .build()
        .unwrap();

    // probably a fancy pure rust way to do this but does it have colors?!
    let mut command_str = OsString::from("ls -a1 ");
    command_str.push(requested_dir.as_os_str());

    let ls_files = std::str::from_utf8(
        &ExecProcess::new("env")
            .arg("sh")
            .arg("-c")
            .arg(command_str)
            .output()?
            .stdout,
    )?
    .to_owned();

    // `SkimItemReader` is a helper to turn any `BufRead` into a stream of `SkimItem`
    // `SkimItem` was implemented for `AsRef<str>` by default
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(Cursor::new(ls_files));

    // `run_with` would read and show items from the stream
    let selected_items = Skim::run_with(&options, Some(items))
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    let res = selected_items
        .iter()
        .map(|i| i.output().to_string())
        .collect();

    Ok(res)
}

pub fn interactive_exec(
    config: &Config,
    requested_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let raw_paths = if config.opt_interactive {
        let res = if config.raw_paths.is_empty() || PathBuf::from(&config.raw_paths[0]).is_dir() {
            interactive_lookup(config, requested_dir)?
        } else if config.raw_paths.len().gt(&1usize) {
            return Err(HttmError::new("May only specify one path in interactive mode.").into());
        } else if !Path::new(&config.raw_paths[0]).is_dir() {
            return Err(
                HttmError::new("Path specified is not a directory suitable for browsing.").into(),
            );
        } else {
            unreachable!("Nope, nope, shouldn't be here!!")
        };
        res
    } else {
        config.raw_paths.clone()
    };

    Ok(raw_paths)
}
