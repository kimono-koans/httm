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

use crate::display::display_exec;
use crate::library::{copy_all, enumerate_directory, paint_string};
use crate::lookup::lookup_exec;
use crate::{Config, DeletedMode, ExecMode, HttmError, InteractiveMode, PathData};

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

pub struct SelectionCandidate {
    config: Arc<Config>,
    path: PathBuf,
}

impl SelectionCandidate {
    pub fn new(config: Arc<Config>, path: PathBuf) -> Self {
        SelectionCandidate { config, path }
    }
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
        let res = preview_view(&self.config, &self.path).unwrap_or_default();
        skim::ItemPreview::AnsiText(res)
    }
}

fn preview_view(
    config: &Config,
    path: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
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
        opt_alt_replicated: config.opt_alt_replicated.to_owned(),
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

pub fn interactive_exec(
    out: &mut Stdout,
    config: &Config,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // go to interactive_select early if user has already requested a file
    // and we are in the appropriate mode Select or Restore, see struct Config
    let vec_pathdata = if config.paths.get(0).is_some() && !&config.paths[0].is_dir() {
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
            if vec_pathdata.is_empty() {
                Err(HttmError::new("Invalid value selected. Quitting.").into())
            } else {
                interactive_select(out, config, &vec_pathdata)?;
                unreachable!()
            }
        }
        // InteractiveMode::Browse executes back through fn exec() in httm.rs
        InteractiveMode::Browse => Ok(vec_pathdata),
        InteractiveMode::None => unreachable!(),
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
    let requested_file_name = select_restore_view(selection_buffer, false)?;
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
    // request is also sanity check for snap path exists below when we check
    // if snap_pathdata is_phantom below
    let snap_pathdata = PathData::from(Path::new(&parsed_str));

    // sanity check -- snap version has good metadata?
    if snap_pathdata.is_phantom {
        return Err(HttmError::new("Snapshot location does not exist on disk. Quitting.").into());
    }

    // sanity check -- snap version is not actually a live copy?
    if config
        .paths
        .clone()
        .into_iter()
        .any(|path| path == snap_pathdata)
    {
        return Err(HttmError::new("Path selected is a 'live' version.  httm will not restore from a live version.  Quitting.").into());
    }

    // build new place to send file
    let old_snap_filename = snap_pathdata
        .path_buf
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let new_snap_filename: String =
        old_snap_filename + ".httm_restored." + &timestamp_file(&snap_pathdata.system_time);

    let new_file_dir = config.pwd.path_buf.clone();
    let new_file_path_buf: PathBuf = [new_file_dir, PathBuf::from(new_snap_filename)]
        .iter()
        .collect();

    // print error on the user selecting to restore the live version of a file
    if new_file_path_buf == snap_pathdata.path_buf {
        return Err(
            HttmError::new("Will not restore files as files are the same file. Quitting.").into(),
        );
    };

    // tell the user what we're up to, and get consent
    let preview_buffer = format!(
        "httm will copy a file from a ZFS snapshot...\n\n\
        \tfrom: {:?}\n\
        \tto:   {:?}\n\n\
        Before httm does anything, it would like your consent. Continue? (YES/NO)\n\
        ─────────────────────────────────────────────────────────────────────────\n\
        YES\n\
        NO",
        snap_pathdata.path_buf, new_file_path_buf
    );

    let res = select_restore_view(preview_buffer, true)?;

    if res == "YES" {
        match copy_all(&snap_pathdata.path_buf, &new_file_path_buf) {
            Ok(_) => writeln!(out, "Restore completed successfully.")?,
            Err(err) => {
                return Err(HttmError::with_context("Restore failed", Box::new(err)).into());
            }
        }
    } else {
        writeln!(out, "User declined.  No files were restored.")?;
    }

    std::process::exit(0)
}

fn lookup_view(
    config: &Config,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prep thread spawn
    let requested_dir_clone = config.requested_dir.path_buf.clone();
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
    let arc_config = Arc::new(config.clone());

    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        let mut out = std::io::stdout();
        let _ = enumerate_directory(arc_config, &tx_item, &requested_dir_clone, &mut out);
    });

    // create the skim component for previews
    let options = SkimOptionsBuilder::default()
        .preview_window(Some("up:50%"))
        .preview(Some(""))
        .header(Some("PREVIEW UP: shift+up | PREVIEW DOWN: shift+down\n\
                      PAGE UP:    page up  | PAGE DOWN:    page down \n\
                      EXIT:       esc      | SELECT:       enter      | SELECT, MULTIPLE: shift+tab\n\
                      ──────────────────────────────────────────────────────────────────────────────",
        ))
        .multi(true)
        .build()
        .unwrap();

    // run_with() reads and shows items from the thread stream created above
    let selected_items = if let Some(output) = Skim::run_with(&options, Some(rx_item)) {
        if output.is_abort {
            eprintln!("httm interactive file browse session was aborted.  Quitting.");
            std::process::exit(0)
        } else {
            output.selected_items
        }
    } else {
        return Err(HttmError::new("httm interactive file browse session failed.").into());
    };

    // output() converts the filename/raw path to a absolute path string for use elsewhere
    let res: Vec<String> = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(res)
}

fn select_restore_view(
    preview_buffer: String,
    reverse: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build our interactive view - less to do than before - no previews, looking through one 'lil buffer
    let skim_opts = SkimOptionsBuilder::default()
        .tac(reverse)
        .nosort(reverse)
        .tabstop(Some("4"))
        .exact(true)
        .multi(false)
        .header(Some(
            "PAGE UP:    page up  | PAGE DOWN:  page down\n\
                      EXIT:       esc      | SELECT:     enter    \n\
                      ─────────────────────────────────────────────",
        ))
        .build()
        .unwrap();

    let item_reader_opts = SkimItemReaderOption::default().ansi(true);
    let item_reader = SkimItemReader::new(item_reader_opts);

    let items = item_reader.of_bufread(Cursor::new(preview_buffer));

    // run_with() reads and shows items from the thread stream created above
    let selected_items = if let Some(output) = Skim::run_with(&skim_opts, Some(items)) {
        if output.is_abort {
            eprintln!("httm select/restore session was aborted.  Quitting.");
            std::process::exit(0)
        } else {
            output.selected_items
        }
    } else {
        return Err(HttmError::new("httm select/restore session failed.").into());
    };

    // output() converts the filename/raw path to a absolute path string for use elsewhere
    let res = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(res)
}

fn timestamp_file(system_time: &SystemTime) -> String {
    let date_time: DateTime<Local> = system_time.to_owned().into();
    format!("{}", date_time.format("%b-%d-%Y-%H:%M:%S"))
}
