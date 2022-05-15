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

use std::fs::FileType;
use std::{ffi::OsString, io::Cursor, path::Path, path::PathBuf, thread, vec};

extern crate skim;
use lscolors::{Colorable, LsColors, Style};

use skim::prelude::*;

use crate::display::display_exec;
use crate::lookup::get_versions;
use crate::recursive::enumerate_directory;
use crate::utility::{copy_recursive, timestamp_file};
use crate::{
    Config, DeletedMode, ExecMode, HttmError, InteractiveMode, PathData,
    ZFS_HIDDEN_SNAPSHOT_DIRECTORY,
};

pub struct SelectionCandidate {
    config: Arc<Config>,
    file_name: OsString,
    path: PathBuf,
    file_type: Option<FileType>,
    is_phantom: bool,
}

impl SelectionCandidate {
    pub fn new(
        config: Arc<Config>,
        file_name: OsString,
        path: PathBuf,
        file_type: Option<FileType>,
        is_phantom: bool,
    ) -> Self {
        SelectionCandidate {
            config,
            file_name,
            path,
            file_type,
            is_phantom,
        }
    }

    fn paint_selection_candidate<'a>(&self, display_name: &'a str) -> Cow<'a, str> {
        let ls_colors = LsColors::from_env().unwrap_or_default();

        if let Some(style) = ls_colors.style_for(self) {
            let ansi_style = &Style::to_ansi_term_style(style);
            Cow::Owned(ansi_style.paint(display_name).to_string())
        } else if !self.is_phantom {
            // if a non-phantom file that should not be colored (regular files)
            Cow::Borrowed(display_name)
        } else if let Some(style) = &Style::from_ansi_sequence("38;2;250;200;200;1;0") {
            // paint all other phantoms/deleted files the same color, light pink
            let ansi_style = &Style::to_ansi_term_style(style);
            Cow::Owned(ansi_style.paint(display_name).to_string())
        } else {
            // just in case if all else fails
            Cow::Borrowed(display_name)
        }
    }

    fn preview_view(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let config = &self.config;
        let path = &self.path;
        // generate a config for a preview display only
        let gen_config = Config {
            paths: vec![PathData::from(path.as_path())],
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
        let snaps_and_live_set = get_versions(&gen_config, &gen_config.paths)?;
        // and display
        let output_buf = display_exec(&gen_config, snaps_and_live_set)?;

        Ok(output_buf)
    }
}

impl Colorable for SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.to_owned()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.file_name.to_owned()
    }
    fn file_type(&self) -> Option<FileType> {
        self.file_type
    }
    fn metadata(&self) -> Option<std::fs::Metadata> {
        self.path.symlink_metadata().ok()
    }
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<str> {
        self.path.file_name().unwrap_or_default().to_string_lossy()
    }
    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(
            &self.paint_selection_candidate(
                &self
                    .path
                    .strip_prefix(&self.config.requested_dir.path_buf)
                    .unwrap_or_else(|_| Path::new(&self.file_name))
                    .to_string_lossy(),
            ),
        )
    }
    fn output(&self) -> Cow<str> {
        self.path.to_string_lossy()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let res = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(res)
    }
}

pub fn interactive_exec(
    config: &Config,
) -> Result<Vec<PathData>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // go to interactive_select early if user has already requested a file
    // and we are in the appropriate mode Select or Restore, see struct Config
    let vec_pathdata = if config.paths.get(0).is_some() && !&config.paths[0].is_dir() {
        // can index here because because we have guaranteed we have this one path
        let selected_file = config.paths[0].to_owned();
        interactive_select(config, &vec![selected_file])?;
        unreachable!()
    } else {
        // collect string paths from what we get from lookup_view
        browse_view(config)?
            .into_iter()
            .map(|path_string| PathData::from(Path::new(&path_string)))
            .collect::<Vec<PathData>>()
    };

    // do we return back to our main exec function to print,
    // or continue down the interactive rabbit hole?
    match config.interactive_mode {
        InteractiveMode::Restore | InteractiveMode::Select => {
            if vec_pathdata.is_empty() {
                Err(HttmError::new("Invalid value selected. Quitting.").into())
            } else {
                interactive_select(config, &vec_pathdata)?;
                unreachable!()
            }
        }
        // InteractiveMode::Browse executes back through fn exec() in httm.rs
        InteractiveMode::Browse => Ok(vec_pathdata),
        InteractiveMode::None => unreachable!(),
    }
}

fn browse_view(
    config: &Config,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // prep thread spawn
    let requested_dir_clone = config.requested_dir.path_buf.clone();
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
    let arc_config = Arc::new(config.clone());

    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        let _ = enumerate_directory(arc_config, &tx_item, &requested_dir_clone);
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

fn interactive_select(
    config: &Config,
    vec_paths: &Vec<PathData>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // same stuff we do at fn exec, snooze...
    let snaps_and_live_set = get_versions(config, vec_paths)?;
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
        Ok(interactive_restore(config, parsed_str)?)
    } else {
        println!("\"{}\"", parsed_str);
        std::process::exit(0)
    }
}

fn select_restore_view(
    preview_buffer: String,
    reverse: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // build our browse view - less to do than before - no previews, looking through one 'lil buffer
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

fn interactive_restore(
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
        return Err(HttmError::new("Source location does not exist on disk. Quitting.").into());
    }

    // sanity check -- snap version is not actually a live copy?
    if !snap_pathdata
        .path_buf
        .to_string_lossy()
        .contains(ZFS_HIDDEN_SNAPSHOT_DIRECTORY)
    {
        return Err(HttmError::new("Path selected is not a 'snapshot version'.  httm will not restore from a non-'snapshot version'.  Quitting.").into());
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

    // don't let the user rewrite one restore over another.
    if new_file_path_buf.exists() {
        return Err(
            HttmError::new("httm will not restore to that file, as a file with the same path name already exists. Quitting.").into(),
        );
    };

    // tell the user what we're up to, and get consent
    let preview_buffer = format!(
        "httm will copy a file from a ZFS snapshot:\n\n\
        \tfrom: {:?}\n\
        \tto:   {:?}\n\n\
        Before httm restores this file, it would like your consent. Continue? (YES/NO)\n\
        ──────────────────────────────────────────────────────────────────────────────\n\
        YES\n\
        NO",
        snap_pathdata.path_buf, new_file_path_buf
    );

    let user_consent = select_restore_view(preview_buffer, true)?;

    if user_consent == "YES" {
        match copy_recursive(&snap_pathdata.path_buf, &new_file_path_buf) {
            Ok(_) => {
                let result_buffer = format!(
                    "httm copied a file from a ZFS snapshot:\n\n\
                    \tfrom: {:?}\n\
                    \tto:   {:?}\n\n\
                    Restore completed successfully.",
                    snap_pathdata.path_buf, new_file_path_buf
                );
                eprintln!("{}", result_buffer);
            }
            Err(err) => {
                return Err(HttmError::with_context("httm restore failed", Box::new(err)).into());
            }
        }
    } else {
        eprintln!("User declined.  No files were restored.");
    }

    std::process::exit(0)
}
