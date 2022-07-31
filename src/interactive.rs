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

use std::{ffi::OsString, fs::FileType, io::Cursor, path::Path, path::PathBuf, thread, vec};

use lscolors::Colorable;
use skim::prelude::*;

use crate::display::display_exec;
use crate::lookup_versions::versions_lookup_exec;
use crate::recursive::recursive_exec;
use crate::utility::{
    copy_recursive, get_date, paint_string, print_output_buf, HttmError, PathData,
};
use crate::{
    Config, DateFormat, DeletedMode, ExecMode, HttmResult, InteractiveMode, RequestRelative,
};

// these represent to items ready for selection and preview
// contains everything needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    config: Arc<Config>,
    file_name: OsString,
    path: PathBuf,
    file_type: Option<FileType>,
    pub is_phantom: bool,
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

    fn preview_view(&self) -> HttmResult<String> {
        let config = &self.config;
        let path = &self.path;
        // generate a config for a preview display only
        let gen_config = Arc::new(Config {
            paths: vec![PathData::from(path.as_path())],
            opt_raw: false,
            opt_zeros: false,
            opt_no_pretty: false,
            opt_recursive: false,
            opt_no_live: false,
            opt_exact: false,
            opt_overwrite: false,
            opt_no_filter: false,
            opt_no_snap: false,
            opt_debug: false,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::Display,
            deleted_mode: DeletedMode::Disabled,
            interactive_mode: InteractiveMode::None,
            datasets_of_interest: config.datasets_of_interest.clone(),
            dataset_collection: config.dataset_collection.clone(),
            pwd: config.pwd.clone(),
            requested_dir: config.requested_dir.clone(),
        });

        // finally run search on those paths
        let snaps_and_live_set = versions_lookup_exec(gen_config.as_ref(), &gen_config.paths)?;
        // and display
        let output_buf = display_exec(gen_config.as_ref(), &snaps_and_live_set)?;

        Ok(output_buf)
    }
}

impl Colorable for &SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.file_name.clone()
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
        AnsiString::parse(&paint_string(
            self,
            &self
                .path
                .strip_prefix(
                    &self
                        .config
                        .requested_dir
                        .as_ref()
                        .expect("requested_dir should never be None in Interactive Browse mode")
                        .path_buf,
                )
                .unwrap_or_else(|_| Path::new(&self.file_name))
                .to_string_lossy(),
        ))
    }
    fn output(&self) -> Cow<str> {
        self.path.to_string_lossy()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let res = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(res)
    }
}

pub fn interactive_exec(config: Arc<Config>) -> HttmResult<Vec<PathData>> {
    let vec_pathdata = match &config.requested_dir {
        // collect string paths from what we get from lookup_view
        Some(requested_dir) => {
            let mut res_vec = Vec::new();

            // loop until user selects a valid path
            while res_vec.is_empty() {
                res_vec = browse_view(config.clone(), requested_dir)?
                    .into_iter()
                    .map(|path_string| PathData::from(Path::new(&path_string)))
                    .collect::<Vec<PathData>>();
            }

            res_vec
        }
        None => {
            // go to interactive_select early if user has already requested a file
            // and we are in the appropriate mode Select or Restore, see struct Config,
            // and None here is also used for LastSnap to skip browsing for a file/dir
            match config.paths.get(0) {
                Some(first_path) => {
                    let selected_file = first_path.clone();
                    interactive_select(config, &[selected_file])?;
                    unreachable!("interactive select never returns so unreachable here")
                }
                // Config::from should never allow us to have an instance where we don't
                // have at least one path to use
                None => unreachable!(
                    "config.paths.get(0) should never be a None value in Interactive Mode"
                ),
            }
        }
    };

    // do we return back to our main exec function to print,
    // or continue down the interactive rabbit hole?
    match config.interactive_mode {
        InteractiveMode::Restore | InteractiveMode::Select => {
            interactive_select(config, &vec_pathdata)?;
            unreachable!()
        }
        // InteractiveMode::Browse executes back through fn exec() in main.rs
        InteractiveMode::Browse => Ok(vec_pathdata),
        InteractiveMode::None => unreachable!(),
    }
}

fn browse_view(config: Arc<Config>, requested_dir: &PathData) -> HttmResult<Vec<String>> {
    // prep thread spawn
    let requested_dir_clone = requested_dir.path_buf.clone();
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = config.clone();

    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        let _ = recursive_exec(config_clone, &tx_item, &requested_dir_clone);
    });

    let opt_multi = !matches!(config.exec_mode, ExecMode::LastSnap(_));

    // create the skim component for previews
    let options = SkimOptionsBuilder::default()
        .preview_window(Some("up:50%"))
        .preview(Some(""))
        .exact(config.opt_exact)
        .header(Some("PREVIEW UP: shift+up | PREVIEW DOWN: shift+down\n\
                      PAGE UP:    page up  | PAGE DOWN:    page down \n\
                      EXIT:       esc      | SELECT:       enter      | SELECT, MULTIPLE: shift+tab\n\
                      ──────────────────────────────────────────────────────────────────────────────",
        ))
        .multi(opt_multi)
        .regex(false)
        .build()
        .expect("Could not initialized skim options for browse_view");

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

fn interactive_select(config: Arc<Config>, vec_paths: &[PathData]) -> HttmResult<()> {
    let snaps_and_live_set = versions_lookup_exec(config.as_ref(), vec_paths)?;

    let path_string = match &config.exec_mode {
        ExecMode::LastSnap(request_relative) => {
            // should be good to index into both, there is a known known 2nd vec,
            let live_version = &vec_paths
                .get(0)
                .expect("ExecMode::LiveSnap should always have exactly one path.");
            let pathdata = match snaps_and_live_set[0]
                .iter()
                .filter(|snap_version| {
                    if request_relative == &RequestRelative::Relative
                        && snap_version.metadata.as_ref().is_some()
                        && live_version.metadata.as_ref().is_some()
                    {
                        snap_version.metadata.as_ref().unwrap().modify_time
                            != live_version.metadata.as_ref().unwrap().modify_time
                    } else {
                        true
                    }
                })
                .last()
            {
                Some(pathdata) => pathdata,
                None => {
                    return Err(HttmError::new(
                        "No last snapshot for the requested input file exists.",
                    )
                    .into())
                }
            };
            pathdata.path_buf.to_string_lossy().into_owned()
        }
        _ => {
            // same stuff we do at fn exec, snooze...
            let selection_buffer = display_exec(config.as_ref(), &snaps_and_live_set)?;
            // get the file name
            let mut requested_file_name = select_restore_view(&selection_buffer, false)?;
            let res_path_string;

            // loop until user selects a valid snapshot version
            loop {
                // ... we want everything between the quotes
                let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
                // ... and the file is the 2nd item or the indexed "1" object
                match broken_string.get(1) {
                    Some(path_from_string)
                        if !snaps_and_live_set[1].iter().any(|pathdata| {
                            pathdata == &PathData::from(Path::new(path_from_string))
                        }) =>
                    {
                        res_path_string = path_from_string.to_string();
                        break;
                    }
                    // Cannot select a 'live' version or other invalid value.  Select another value.
                    _ => {
                        requested_file_name = select_restore_view(&selection_buffer, false)?;
                    }
                }
            }

            res_path_string
        }
    };

    // continue to interactive_restore or print and exit here?
    if config.interactive_mode == InteractiveMode::Restore {
        Ok(interactive_restore(config, &path_string, vec_paths)?)
    } else {
        let output_buf = format!("\"{}\"\n", &path_string);
        print_output_buf(output_buf)?;
        std::process::exit(0)
    }
}

fn select_restore_view(preview_buffer: &str, reverse: bool) -> HttmResult<String> {
    // build our browse view - less to do than before - no previews, looking through one 'lil buffer
    let skim_opts = SkimOptionsBuilder::default()
        .tac(reverse)
        .nosort(reverse)
        .tabstop(Some("4"))
        .exact(true)
        .multi(false)
        .regex(false)
        .header(Some(
            "PAGE UP:    page up  | PAGE DOWN:  page down\n\
                      EXIT:       esc      | SELECT:     enter    \n\
                      ─────────────────────────────────────────────",
        ))
        .build()
        .expect("Could not initialized skim options for select_restore_view");

    let item_reader_opts = SkimItemReaderOption::default().ansi(true);
    let item_reader = SkimItemReader::new(item_reader_opts);

    let items = item_reader.of_bufread(Cursor::new(preview_buffer.to_owned()));

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
    config: Arc<Config>,
    parsed_str: &str,
    vec_paths: &[PathData],
) -> HttmResult<()> {
    // build pathdata from selection buffer parsed string
    //
    // request is also sanity check for snap path exists below when we check
    // if snap_pathdata is_phantom below
    let snap_pathdata = PathData::from(Path::new(&parsed_str));

    // sanity check -- snap version has good metadata?
    let snap_path_metadata = snap_pathdata
        .metadata
        .ok_or_else(|| HttmError::new("Source location does not exist on disk. Quitting."))?;

    // build new place to send file
    let new_file_path_buf = if config.opt_overwrite {
        // corner case: what if multiple selected paths had the same file name,
        // but were in different directories? let's make sure we have only one match
        let opt_og_pathdata = vec_paths.iter().find_map(|pathdata| {
            match versions_lookup_exec(config.as_ref(), &[pathdata.clone()]).ok() {
                // safe to index into snaps, known len of 2 for set
                Some(pathdata_set) => pathdata_set[0].iter().find_map(|pathdata| {
                    if pathdata == &snap_pathdata {
                        // safe to index into request, known len of 2 for set, known len of 1 for request
                        let og_pathdata = pathdata_set[1][0].to_owned();
                        Some(og_pathdata)
                    } else {
                        None
                    }
                }),
                None => None,
            }
        });

        match opt_og_pathdata {
            Some(pathdata) => pathdata.path_buf,
            None => {
                return Err(HttmError::new(
                    "httm unable to determine original file path in overwrite mode.  Quitting.",
                )
                .into())
            }
        }
    } else {
        let snap_filename = snap_pathdata
            .path_buf
            .file_name()
            .expect("Could not obtain a file name for the snap file version of path given")
            .to_string_lossy()
            .into_owned();

        let new_filename = snap_filename
            + ".httm_restored."
            + &get_date(
                &config,
                &snap_path_metadata.modify_time,
                DateFormat::Timestamp,
            );
        let new_file_dir = config.pwd.path_buf.clone();
        let new_file_path_buf: PathBuf = new_file_dir.join(new_filename);

        // don't let the user rewrite one restore over another in non-overwrite mode
        if new_file_path_buf.exists() {
            return Err(
                HttmError::new("httm will not restore to that file, as a file with the same path name already exists. Quitting.").into(),
            );
        } else {
            new_file_path_buf
        }
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

    let mut user_consent = select_restore_view(&preview_buffer, true)?.to_ascii_uppercase();

    // loop until user consents or doesn't
    loop {
        match user_consent.as_ref() {
            "YES" | "Y" => {
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
                        return Err(HttmError::with_context(
                            "httm restore failed for the following reason",
                            Box::new(err),
                        )
                        .into());
                    }
                }
                break;
            }
            "NO" | "N" => {
                eprintln!("User declined restore.  No files were restored.");
                break;
            }
            _ => {
                user_consent = select_restore_view(&preview_buffer, true)?.to_ascii_uppercase();
            }
        }
    }

    std::process::exit(0)
}
