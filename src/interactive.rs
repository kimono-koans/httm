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

use crate::deleted::get_deleted;
use crate::display::{display_exec, paint_string};
use crate::lookup::lookup_exec;
use crate::{read_stdin, Config, HttmError, InteractiveMode, PathData, SnapPoint};

extern crate skim;
use chrono::{DateTime, Local};
use rayon::prelude::*;
use skim::prelude::*;
use std::{
    ffi::OsStr,
    io::{Cursor, Stdout, Write as IoWrite},
    path::Path,
    path::PathBuf,
    thread,
    time::SystemTime,
    vec,
};

struct SelectionCandidate {
    path: PathBuf,
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<str> {
        self.path
            .file_name()
            .unwrap_or_else(|| OsStr::new(""))
            .to_string_lossy()
    }
    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&paint_string(
            &self.path,
            &self
                .path
                .file_name()
                .unwrap_or_else(|| OsStr::new(""))
                .to_string_lossy(),
        ))
    }
    fn output(&self) -> Cow<str> {
        let path = self
            .path
            .canonicalize()
            .unwrap_or_else(|_| self.path.clone())
            .to_string_lossy()
            .into_owned();
        Cow::Owned(path)
    }
}

pub fn interactive_exec(
    out: &mut Stdout,
    config: &Config,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // go to interactive_select early if user has already requested a file
    // and we are in the appropriate mode Select or Restore, see struct Config
    let vec_pathdata = if config.paths.len() == 1 && config.paths[0].path_buf.is_file() {
        let selected_file = config.paths[0].to_owned();
        interactive_select(out, config, &vec![selected_file])?;
        unreachable!()
    } else {
        // collect string paths from what we get from lookup_view
        let paths: Vec<PathData> = lookup_view(config)?
            .iter()
            .map(Path::new)
            .map(PathData::new)
            .collect();
        paths
    };

    // do we return back to our main exec function to print,
    // or continue down the interactive rabbit hole?
    match config.interactive_mode {
        InteractiveMode::Restore | InteractiveMode::Select => {
            interactive_select(out, config, &vec_pathdata)?;
            unreachable!()
        }
        // InteractiveMode::Lookup, etc., executes back through fn exec() in httm.rs
        _ => Ok(vec_pathdata),
    }
}

fn interactive_select(
    out: &mut Stdout,
    config: &Config,
    vec_paths: &Vec<PathData>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // same stuff we do at exec, snooze...
    let snaps_and_live_set = lookup_exec(config, vec_paths)?;
    let selection_buffer = display_exec(config, snaps_and_live_set)?;

    // get the file name, and get ready to do some file ops!!
    let requested_file_name = select_view(selection_buffer)?;
    // ... we want everything between the quotes
    let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
    // ... and the file is the 2nd item or the indexed "1" object
    let parsed_str = if let Some(parsed) = broken_string.get(1) {
        parsed
    } else {
        return Err(HttmError::new("Invalid value selected. Quitting.").into());
    };

    // continue to interactive_restore or print and exit here?
    if config.interactive_mode == InteractiveMode::Restore {
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build pathdata from selection buffer parsed string
    //
    // request is also sanity check for metadata
    let snap_pd = PathData::new(&PathBuf::from(&parsed_str));

    if snap_pd.is_phantom {
        return Err(HttmError::new("Snapshot location does not exist on disk. Quitting.").into());
    };

    // build new place to send file
    let old_snap_filename = snap_pd
        .path_buf
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let new_snap_filename: String =
        old_snap_filename + ".httm_restored." + &timestamp_file(&snap_pd.system_time);

    let new_file_dir = config.pwd.clone();
    let new_file_path_buf: PathBuf = [new_file_dir, PathBuf::from(new_snap_filename)]
        .iter()
        .collect();

    // print error on the user selecting to restore the live version of a file
    if new_file_path_buf == snap_pd.path_buf {
        return Err(
            HttmError::new("Will not restore files as files are the same file. Quitting.").into(),
        );
    };

    // tell the user what we're up to, and get consent
    write!(out, "httm will copy a file from a ZFS snapshot...\n\n")?;
    writeln!(out, "\tfrom: {:?}", snap_pd.path_buf)?;
    writeln!(out, "\tto:   {:?}\n", new_file_path_buf)?;
    write!(
        out,
        "Before httm does anything, it would like your consent. Continue? (Y/N) "
    )?;
    out.flush()?;

    let input_buffer = read_stdin()?;
    let res = input_buffer
        .get(0)
        .unwrap_or(&"N".to_owned())
        .to_lowercase();

    if res == "y" || res == "yes" {
        std::fs::copy(snap_pd.path_buf, new_file_path_buf)?;
        write!(out, "\nRestore completed successfully.\n")?;
    } else {
        write!(out, "\nUser declined.  No files were restored.\n")?;
    }

    std::process::exit(0)
}

fn lookup_view(
    config: &Config,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // We *can* build a preview() method on our SkimItem to do this, except, right now, it's slower
    // because it blocks on preview(), given the implementation of skim, see the new_preview branch

    // prep thread spawn
    let requested_dir_clone = config.requested_dir.path_buf.clone();
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = config.clone();

    // spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        let _ = enumerate_directory(&config_clone, &tx_item, &requested_dir_clone);
    });

    // as skim is slower if we call as a function, we locate which httm command to use in struct Config
    let httm_command = &config.self_command;

    // create command to use for preview, as noted, unable to use a function for now
    let preview_str = match &config.snap_point {
        SnapPoint::UserDefined(defined_dirs) => {
            let snap_point = defined_dirs.snap_dir.to_string_lossy();
            let local_dir = defined_dirs.local_dir.to_string_lossy();

            format!(
                "\"{httm_command}\" --snap-point \"{snap_point}\" --local-dir \"{local_dir}\" {{}}"
            )
        }
        SnapPoint::Native(_) => {
            format!("\"{httm_command}\" {{}}")
        }
    };

    // create the skim component for previews
    let options = SkimOptionsBuilder::default()
        .preview_window(Some("70%"))
        .preview(Some(&preview_str))
        .multi(true)
        .exact(true)
        .build()
        .unwrap();

    // run_with() reads and shows items from the thread stream created above
    let selected_items = Skim::run_with(&options, Some(rx_item))
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    // output() converts the filename/raw path to a absolute path string for use elsewhere
    let res: Vec<String> = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(res)
}

fn select_view(
    selection_buffer: String,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // take what lookup gave us and select from among the snapshot options
    // build our skim view - less to do than before - no previews, looking through one 'lil buffer
    let options = SkimOptionsBuilder::default()
        .interactive(true)
        .exact(true)
        .multi(false)
        .build()
        .unwrap();
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(Cursor::new(selection_buffer));
    let selected_items = Skim::run_with(&options, Some(items))
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    // output() converts the filename/raw path to a absolute path string for use elsewhere
    let res = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(res)
}

fn enumerate_directory(
    config: &Config,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let read_dir = std::fs::read_dir(&requested_dir)?;

    // convert to paths, and split into dirs and files
    let (vec_dirs, vec_files): (Vec<PathBuf>, Vec<PathBuf>) = read_dir
        .filter_map(|i| i.ok())
        .map(|dir_entry| dir_entry.path())
        .partition(|path| path.is_dir());

    let vec_deleted = if config.opt_deleted {
        get_deleted(config, requested_dir)?
            .par_iter()
            .map(|path| path.path_buf.file_name())
            .flatten()
            .map(|str| requested_dir.join(str))
            .collect()
    } else {
        Vec::new()
    };

    // combine dirs and files into a vec and sort to display
    let mut combined_vec: Vec<&PathBuf> = vec![&vec_files, &vec_dirs, &vec_deleted]
        .into_par_iter()
        .flatten()
        .collect();
    combined_vec.par_sort();
    // don't want a par_iter here because it will block and wait for all
    // results, instead of printing and recursing into the subsequent dirs
    combined_vec.iter().for_each(|path| {
        let _ = tx_item.send(Arc::new(SelectionCandidate {
            path: path.to_path_buf(),
        }));
    });

    // now recurse into those dirs, if requested
    if config.opt_recursive {
        vec_dirs
            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            .iter()
            .for_each(|requested_dir| {
                let _ = enumerate_directory(config, tx_item, requested_dir);
            })
    }
    Ok(())
}

fn timestamp_file(st: &SystemTime) -> String {
    let dt: DateTime<Local> = st.to_owned().into();
    format!("{}", dt.format("%b-%d-%Y-%H:%M:%S"))
}
