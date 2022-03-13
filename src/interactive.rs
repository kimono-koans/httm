use crate::convert_strings_to_pathdata;
use crate::display::{display_path_colors, display_pretty};
use crate::lookup::run_search;
use crate::read_stdin;
use crate::Config;
use crate::HttmError;

extern crate skim;
use chrono::DateTime;
use chrono::Local;
use skim::prelude::*;
use std::fs::ReadDir;
use std::io::Cursor;
use std::io::Write;
use std::time::SystemTime;
use std::vec;

use std::io::Stdout;
use std::{
    fmt::Write as FmtWrite,
    path::{Path, PathBuf},
};

fn lookup_view(config: &Config) -> Result<String, Box<dyn std::error::Error>> {
    // build our paths for the httm preview invocations, we need Strings because for now
    // skim does not allow invoking preview from a Rust function, we actually just exec httm again
    let local_dir = if let Some(local_dir) = &config.opt_local_dir {
        local_dir.to_string_lossy()
    } else {
        config.current_working_dir.to_string_lossy()
    };

    let can_path = if let Ok(can_path) = config.user_requested_dir.canonicalize() {
        can_path
    } else {
        config.current_working_dir.clone()
    };

    let mut buff = String::new();
    let mut read_dir = std::fs::read_dir(&can_path)?;
    let cp_string = can_path.to_string_lossy();

    // enter directory
    enter_directory(config, &mut buff, &mut read_dir, &can_path);

    // string to exec on each preview
    let preview_str = if let Some(sp_os) = &config.opt_snap_point {
        let snap_point = sp_os.to_string_lossy();
        format!(
            "httm --snap-point \"{snap_point}\" --local-dir \"{local_dir}\" \"{}\"/{{}}",
            cp_string
        )
    } else {
        format!("httm \"{}\"/{{}}", cp_string)
    };

    let options = SkimOptionsBuilder::default()
        .preview_window(Some("70%"))
        .preview(Some(&preview_str))
        .build()
        .unwrap();

    // `SkimItemReader` is a helper to turn any `BufRead` into a stream of `SkimItem`
    // `SkimItem` was implemented for `AsRef<str>` by default
    let reader_opt = SkimItemReaderOption::default().ansi(true);
    let item_reader = SkimItemReader::new(reader_opt);
    let items = item_reader.of_bufread(Cursor::new(buff));

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

fn select_view(selection_buffer: String) -> Result<String, Box<dyn std::error::Error>> {
    let options = SkimOptionsBuilder::default()
        .interactive(true)
        .build()
        .unwrap();

    // `SkimItemReader` is a helper to turn any `BufRead` into a stream of `SkimItem`
    // `SkimItem` was implemented for `AsRef<str>` by default
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(Cursor::new(selection_buffer));

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

fn enter_directory(config: &Config, buff: &mut String, read_dir: &mut ReadDir, can_path: &Path) {
    let mut vec_dir = Vec::new();
    let mut vec_files = Vec::new();

    // convert to paths
    for raw_entry in read_dir {
        let dir_entry = if let Ok(dir_entry) = raw_entry {
            dir_entry
        } else {
            continue;
        };
        let path = dir_entry.path();

        if path.is_dir() {
            vec_dir.push(path);
        } else if path.is_file() || path.is_symlink() {
            vec_files.push(path);
        }
    }

    // display with pretty ANSI colors
    let mut combined_vec = vec_dir.clone();
    combined_vec.append(&mut vec_files);
    combined_vec.sort();
    for path in combined_vec {
        let _ = writeln!(buff, "{}", display_path_colors(&path, can_path));
    }

    // now recurse, if requested
    if config.opt_recursive {
        for dir in vec_dir {
            let mut read_dir = if let Ok(read_dir) = std::fs::read_dir(dir) {
                read_dir
            } else {
                continue;
            };
            enter_directory(config, buff, &mut read_dir, can_path);
        }
    }
}

pub fn interactive_exec(
    out: &mut Stdout,
    config: &Config,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let raw_paths = if config.opt_interactive {
        let res = if config.raw_paths.is_empty() || PathBuf::from(&config.raw_paths[0]).is_dir() {
            vec![lookup_view(config)?]
        } else if config.raw_paths.len().gt(&1usize) {
            return Err(HttmError::new("May only specify one path in interactive mode.").into());
        } else if !Path::new(&config.raw_paths[0]).is_dir() {
            return Err(
                HttmError::new("Path specified is not a directory suitable for browsing.").into(),
            );
        } else {
            unreachable!("Nope, nope, you shouldn't be here!!  Just kidding, file a bug if you find yourself here.")
        };
        res
    } else {
        config.raw_paths.clone()
    };

    if config.opt_restore || config.opt_select {
        interactive_select(out, config, raw_paths)?;
        unreachable!("You *really* shouldn't be here!!! No.... no.... AHHHHHHHHGGGGG... Just kidding, file a bug if you find yourself here.")
    } else {
        Ok(raw_paths)
    }
}

fn interactive_select(
    out: &mut Stdout,
    config: &Config,
    raw_paths: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // same stuff we do at exec, snooze...
    let search_path = raw_paths.get(0).unwrap().to_owned();
    let pathdata_set = convert_strings_to_pathdata(config, &[search_path])?;
    let snaps_and_live_set = run_search(config, pathdata_set)?;
    let selection_buffer = display_pretty(config, snaps_and_live_set)?;

    // file name ready to do some file ops!!
    // ... we want the 2nd item or the indexed "1" object
    // everything between the quotes
    let requested_file_name = select_view(selection_buffer)?;
    let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
    let parsed_str = if let Some(parsed) = broken_string.get(1) {
        parsed
    } else {
        return Err(HttmError::new("Invalid value selected. Quitting.").into());
    };

    if config.opt_restore {
        Ok(interactive_restore(out, config, parsed_str)?)
    } else {
        writeln!(out, "\"{}\"", parsed_str)?;
        std::process::exit(0)
    }
}

fn interactive_restore(
    out: &mut Stdout,
    config: &Config,
    parsed_str: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let snap_pbuf = PathBuf::from(&parsed_str);

    let snap_md = if let Ok(snap_md) = snap_pbuf.metadata() {
        snap_md
    } else {
        return Err(HttmError::new("Snapshot location does not exist on disk. Quitting.").into());
    };

    // build new place to send file
    let old_snap_filename = snap_pbuf.file_name().unwrap().to_string_lossy().to_string();
    let new_snap_filename: String =
        old_snap_filename.clone() + ".httm_restored." + &timestamp_file(&snap_md.modified()?);

    let new_file_dir = config.current_working_dir.clone();
    let new_file_pbuf: PathBuf = [new_file_dir, PathBuf::from(new_snap_filename)]
        .iter()
        .collect();

    let old_file_dir = config.current_working_dir.clone();
    let old_file_pbuf: PathBuf = [old_file_dir, PathBuf::from(old_snap_filename)]
        .iter()
        .collect();

    if old_file_pbuf == snap_pbuf {
        return Err(
            HttmError::new("Will not restore files as files are the same file. Quitting.").into(),
        );
    };

    // tell the user what we're up to
    write!(out, "httm will copy a file from a ZFS snapshot...\n\n")?;
    writeln!(out, "\tfrom: {:?}", snap_pbuf)?;
    writeln!(out, "\tto:   {:?}\n", new_file_pbuf)?;
    write!(
        out,
        "This action is a *non-destructive* copy, but, before httm does anything, it would like your consent. Continue? (Y/N) "
    )?;
    out.flush()?;

    let input_buffer = read_stdin()?;
    let res = input_buffer
        .get(0)
        .unwrap_or(&"N".to_owned())
        .to_lowercase();

    if res == "y" || res == "yes" {
        std::fs::copy(snap_pbuf, new_file_pbuf)?;
        write!(out, "\nRestore completed successfully.\n")?;
    } else {
        write!(out, "\nUser declined.  No files were restored.\n")?;
    }

    std::process::exit(0)
}

fn timestamp_file(st: &SystemTime) -> String {
    let dt: DateTime<Local> = st.to_owned().into();
    format!("{}", dt.format("%b-%d-%H:%M:%S-%Y"))
}
