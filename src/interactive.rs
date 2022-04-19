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
use crate::{read_stdin, Config, DeletedMode, ExecMode, HttmError, InteractiveMode, PathData};

extern crate skim;
use chrono::{DateTime, Local};
use rayon::{iter::Either, prelude::*};
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
    config: Arc<Config>,
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
        let path = self.path.to_string_lossy().into_owned();
        Cow::Owned(path)
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let config = self.config.clone();
        let path = self.path.clone();

        let res = preview_view(&config, &path).unwrap_or_default();
        skim::ItemPreview::AnsiText(res)
    }
}

pub fn interactive_exec(
    out: &mut Stdout,
    config: &Config,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // go to interactive_select early if user has already requested a file
    // and we are in the appropriate mode Select or Restore, see struct Config
    let vec_pathdata = if config.paths.get(0).is_some() && !config.paths[0].path_buf.is_dir() {
        // can index here because because we have guaranteed we have this one path
        let selected_file = config.paths[0].to_owned();
        interactive_select(out, config, &vec![selected_file])?;
        unreachable!()
    } else {
        // collect string paths from what we get from lookup_view
        lookup_view(config)?
            .into_par_iter()
            .map(|string| PathBuf::from(&string))
            .map(|path| PathData::from(path.as_path()))
            .collect::<Vec<PathData>>()
    };

    // do we return back to our main exec function to print,
    // or continue down the interactive rabbit hole?
    match config.interactive_mode {
        InteractiveMode::Restore | InteractiveMode::Select => {
            interactive_select(out, config, &vec_pathdata)?;
            unreachable!()
        }
        // InteractiveMode::Browse, etc., executes back through fn exec() in httm.rs
        _ => Ok(vec_pathdata),
    }
}

fn interactive_select(
    out: &mut Stdout,
    config: &Config,
    vec_paths: &Vec<PathData>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // same stuff we do at fn exec, snooze...
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
    let snap_pd = PathData::from(Path::new(&parsed_str));

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
    let config_clone = Arc::new(config.clone());

    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        let _ = enumerate_directory(config_clone, &tx_item, &requested_dir_clone);
    });

    // create the skim component for previews
    let options = SkimOptionsBuilder::default()
        .preview_window(Some("75%"))
        .preview(Some(""))
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

fn preview_view(
    config: &Config,
    path: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build a config just for previews
    let gen_config = Config {
        paths: vec![PathData::from(path)],
        opt_raw: false,
        opt_zeros: false,
        opt_no_pretty: false,
        opt_recursive: false,
        opt_no_live_vers: false,
        exec_mode: ExecMode::Display,
        deleted_mode: DeletedMode::Disabled,
        interactive_mode: InteractiveMode::None,
        snap_point: config.snap_point.to_owned(),
        pwd: config.pwd.to_owned(),
        requested_dir: config.requested_dir.to_owned(),
    };

    // finally run search on those paths
    let snaps_and_live_set = lookup_exec(&gen_config, &gen_config.paths)?;
    // and display
    let output_buf = display_exec(config, snaps_and_live_set)?;

    Ok(output_buf)
}

fn select_view(
    selection_buffer: String,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // take what lookup gave us and select from among the snapshot options
    // build our skim view - less to do than before - no previews, looking through one 'lil buffer
    let skim_opts = SkimOptionsBuilder::default()
        .interactive(true)
        .exact(true)
        .multi(false)
        .build()
        .unwrap();

    let item_reader_opts = SkimItemReaderOption::default().ansi(true);
    let item_reader = SkimItemReader::new(item_reader_opts);
    let items = item_reader.of_bufread(Cursor::new(selection_buffer));

    let selected_items = Skim::run_with(&skim_opts, Some(items))
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
    config: Arc<Config>,
    tx_item: &SkimItemSender,
    requested_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // Why is this so complicated?  The dir entry file_type() call is inexpensive,
    // so split into dirs and files here and then to paths, and must appropriately
    // handle symlinks to dirs to avoid recursive symlinks as well
    let (vec_dirs, vec_files): (Vec<PathBuf>, Vec<PathBuf>) = std::fs::read_dir(&requested_dir)?
        .flatten()
        .par_bridge()
        .partition_map(|dir_entry| {
            let path = dir_entry.path();
            match dir_entry.file_type() {
                Ok(file_type) => match file_type {
                    file_type if file_type.is_dir() => Either::Left(path),
                    file_type if file_type.is_file() => Either::Right(path),
                    file_type if file_type.is_symlink() => {
                        match path.read_link() {
                            Ok(link) => {
                                // read_link() will check symlink is pointing to a directory
                                //
                                // checking ancestors() against the read_link() will reduce/remove
                                // infinitely recursive paths, like /usr/bin/X11 pointing to /usr/X11
                                if link.is_dir()
                                    && link.ancestors().all(|ancestor| ancestor != link)
                                {
                                    Either::Left(path)
                                } else {
                                    Either::Right(path)
                                }
                            }
                            // we get an error? still pass the path on, as we get a good path from the dir entry
                            Err(_) => Either::Right(path),
                        }
                    }
                    // char, block, etc devices(?) to the right
                    _ => Either::Right(path),
                },
                // we get an error? still pass the path on, as we get a good path from the dir entry
                Err(_) => Either::Right(path),
            }
        });

    let vec_deleted = if config.deleted_mode != DeletedMode::Disabled {
        // why do we do this?  B/c the files are deleted we are recreating what their previous names
        // once were.  Here, we are receiving snap metadata, what would its live version look like if
        // it had not been deleted
        get_deleted(&config, requested_dir)?
            .par_iter()
            .map(|path| path.path_buf.file_name())
            .flatten()
            .map(|str| requested_dir.join(str))
            .collect()
    } else {
        Vec::new()
    };

    // combine dirs and files into a vec and sort to display
    let mut combined_vec: Vec<&PathBuf> = match config.deleted_mode {
        DeletedMode::Only => vec![&vec_deleted].into_par_iter().flatten().collect(),
        DeletedMode::Enabled => vec![&vec_files, &vec_dirs, &vec_deleted]
            .into_par_iter()
            .flatten()
            .collect(),
        DeletedMode::Disabled => vec![&vec_files, &vec_dirs]
            .into_par_iter()
            .flatten()
            .collect(),
    };

    combined_vec.par_sort_unstable_by(|a, b| a.cmp(b));
    // don't want a par_iter here because it will block and wait for all
    // results, instead of printing and recursing into the subsequent dirs
    combined_vec.iter().for_each(|path| {
        let _ = tx_item.send(Arc::new(SelectionCandidate {
            config: config.clone(),
            path: path.to_path_buf(),
        }));
    });

    // now recurse into those dirs, if requested
    if config.opt_recursive {
        vec_dirs
            // don't want a par_iter here because it will block and wait for all
            // results, instead of printing and recursing into the subsequent dirs
            .iter()
            .for_each(move |requested_dir| {
                let config_clone = config.clone();
                let _ = enumerate_directory(config_clone, tx_item, requested_dir);
            });
    }
    Ok(())
}

fn timestamp_file(system_time: &SystemTime) -> String {
    let date_time: DateTime<Local> = system_time.to_owned().into();
    format!("{}", date_time.format("%b-%d-%Y-%H:%M:%S"))
}
