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

use std::{fs::FileType, io::Cursor, path::Path, path::PathBuf, thread, vec};

use lscolors::Colorable;
use skim::prelude::*;

use crate::config::init::Config;
use crate::config::init::{DeletedMode, ExecMode, InteractiveMode, RequestRelative};
use crate::data::paths::{BasicDirEntryInfo, PathData};
use crate::exec::display::display_exec;
use crate::exec::recursive::recursive_exec;
use crate::library::utility::{
    copy_recursive, get_date, map_to_snaps_live_set, paint_string, print_output_buf, DateFormat,
    HttmError, HttmResult,
};
use crate::lookup::versions::versions_lookup_exec;

// these represent the items ready for selection and preview
// contains everything one needs to request preview and paint with
// LsColors -- see preview_view, preview for how preview is done
// and impl Colorable for how we paint the path strings
pub struct SelectionCandidate {
    config: Arc<Config>,
    path: PathBuf,
    file_type: Option<FileType>,
}

impl SelectionCandidate {
    pub fn new(
        config: Arc<Config>,
        basic_dir_entry_info: BasicDirEntryInfo,
        is_phantom: bool,
    ) -> Self {
        SelectionCandidate {
            config,
            path: basic_dir_entry_info.path,
            // here save space of bool/padding instead of an "is_phantom: bool"
            //
            // issue: conflate not having a file_type as phantom
            // for purposes of coloring the file_name/path only?
            //
            // std lib docs don't give much indication as to
            // when file_type() fails?  Doesn't seem to be a problem?
            file_type: {
                if is_phantom {
                    None
                } else {
                    basic_dir_entry_info.file_type
                }
            },
        }
    }

    fn preview_view(&self) -> HttmResult<String> {
        let config = &self.config;
        let path = &self.path;
        // generate a config for a preview display only
        let gen_config = Config {
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
            opt_no_traverse: false,
            opt_unique: false,
            opt_omit_identical: config.opt_omit_identical,
            requested_utc_offset: config.requested_utc_offset,
            exec_mode: ExecMode::Display,
            deleted_mode: DeletedMode::Disabled,
            dataset_collection: config.dataset_collection.clone(),
            pwd: config.pwd.clone(),
            opt_requested_dir: config.opt_requested_dir.clone(),
        };

        // finally run search on those paths
        let map_live_to_snaps = versions_lookup_exec(&gen_config, &gen_config.paths)?;
        // and display
        let output_buf = display_exec(&gen_config, map_live_to_snaps)?;

        Ok(output_buf)
    }
}

impl Colorable for &SelectionCandidate {
    fn path(&self) -> PathBuf {
        self.path.to_path_buf()
    }
    fn file_name(&self) -> std::ffi::OsString {
        self.path.file_name().unwrap_or_default().to_os_string()
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
        self.path.to_string_lossy()
    }
    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&paint_string(
            self,
            &self
                .path
                .strip_prefix(
                    &self
                        .config
                        .opt_requested_dir
                        .as_ref()
                        .expect("requested_dir should never be None in Interactive Browse mode")
                        .path_buf,
                )
                .unwrap_or(&self.path)
                .to_string_lossy(),
        ))
    }
    fn output(&self) -> Cow<str> {
        self.text()
    }
    fn preview(&self, _: PreviewContext<'_>) -> skim::ItemPreview {
        let preview_output = self.preview_view().unwrap_or_default();
        skim::ItemPreview::AnsiText(preview_output)
    }
}

pub fn interactive_exec(
    config: Arc<Config>,
    interactive_mode: &InteractiveMode,
) -> HttmResult<Vec<PathData>> {
    let paths_selected_in_browse = match &config.opt_requested_dir {
        // collect string paths from what we get from lookup_view
        Some(requested_dir) => {
            // loop until user selects a valid path
            loop {
                let selected_pathdata =
                    browse_view(config.clone(), requested_dir, interactive_mode)?
                        .into_iter()
                        .map(|path_string| PathData::from(Path::new(&path_string)))
                        .collect::<Vec<PathData>>();
                if !selected_pathdata.is_empty() {
                    break selected_pathdata;
                }
            }
        }
        None => {
            // go to interactive_select early if user has already requested a file
            // and we are in the appropriate mode Select or Restore, see struct Config,
            // and None here is also used for LastSnap to skip browsing for a file/dir
            match config.paths.get(0) {
                Some(first_path) => {
                    let selected_file = first_path.clone();
                    interactive_select(config, &[selected_file], interactive_mode)?;
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
    match interactive_mode {
        InteractiveMode::LastSnap(_) | InteractiveMode::Restore | InteractiveMode::Select => {
            interactive_select(config, &paths_selected_in_browse, interactive_mode)?;
            unreachable!()
        }
        // InteractiveMode::Browse executes back through fn exec() in main.rs
        InteractiveMode::Browse => Ok(paths_selected_in_browse),
    }
}

fn browse_view(
    config: Arc<Config>,
    requested_dir: &PathData,
    interactive_mode: &InteractiveMode,
) -> HttmResult<Vec<String>> {
    // prep thread spawn
    let requested_dir_clone = requested_dir.path_buf.clone();
    let config_clone = config.clone();
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();

    // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
    thread::spawn(move || {
        // no way to propagate error from closure so exit and explain error here
        recursive_exec(config_clone, &requested_dir_clone, tx_item.clone()).unwrap_or_else(
            |error| {
                eprintln!("Error: {}", error);
                std::process::exit(1)
            },
        )
    });

    let opt_multi = !matches!(interactive_mode, InteractiveMode::LastSnap(_));

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
    let output: Vec<String> = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(output)
}

fn interactive_select(
    config: Arc<Config>,
    paths_selected_in_browse: &[PathData],
    interactive_mode: &InteractiveMode,
) -> HttmResult<()> {
    let map_live_to_snaps = versions_lookup_exec(config.as_ref(), paths_selected_in_browse)?;
    let snaps_and_live_set = map_to_snaps_live_set(&map_live_to_snaps);

    // snap and live set has no snaps
    if snaps_and_live_set[0].is_empty() {
        let paths: Vec<String> = paths_selected_in_browse
            .iter()
            .map(|path| path.path_buf.to_string_lossy().to_string())
            .collect();
        let msg = format!(
            "{}{:?}",
            "Cannot select or restore from the following paths as they have no snapshots:\n", paths
        );
        return Err(HttmError::new(&msg).into());
    }

    let path_string = match &interactive_mode {
        InteractiveMode::LastSnap(request_relative) => {
            // should be good to index into both, there is a known known 2nd vec,
            let live_version = &paths_selected_in_browse
                .get(0)
                .expect("ExecMode::LiveSnap should always have exactly one path.");
            let path_string = snaps_and_live_set[0]
                .iter()
                .filter(|snap_version| {
                    if request_relative == &RequestRelative::Relative {
                        snap_version.md_infallible().modify_time
                            != live_version.md_infallible().modify_time
                    } else {
                        true
                    }
                })
                .last()
                .ok_or_else(|| {
                    HttmError::new("No last snapshot for the requested input file exists.")
                })?
                .path_buf
                .to_string_lossy()
                .into_owned();
            path_string
        }
        _ => {
            // same stuff we do at fn exec, snooze...
            let selection_buffer = display_exec(config.as_ref(), map_live_to_snaps)?;

            // loop until user selects a valid snapshot version
            loop {
                // get the file name
                let requested_file_name = select_restore_view(&selection_buffer, false)?;
                // ... we want everything between the quotes
                let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
                // ... and the file is the 2nd item or the indexed "1" object
                if let Some(path_string) = broken_string.get(1) {
                    // and cannot select a 'live' version or other invalid value.
                    if snaps_and_live_set[1].iter().all(|live_version| {
                        Path::new(path_string) != live_version.path_buf.as_path()
                    }) {
                        // return string from the loop
                        break path_string.to_string();
                    }
                }
            }
        }
    };

    // continue to interactive_restore or print and exit here?
    if matches!(interactive_mode, InteractiveMode::Restore) {
        // one only allow one to select one path string during select
        // but we retain paths_selected_in_browse because we may need
        // it later during restore if opt_overwrite is selected
        Ok(interactive_restore(
            config,
            &path_string,
            paths_selected_in_browse,
        )?)
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
    let output = selected_items
        .iter()
        .map(|i| i.output().into_owned())
        .collect();

    Ok(output)
}

fn interactive_restore(
    config: Arc<Config>,
    parsed_str: &str,
    paths_selected_in_browse: &[PathData],
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
        // instead of just not naming the new file with extra info (date plus "httm_restored") and shoving that new file
        // into the pwd, here, we actually look for the original location of the file to make sure we overwrite it.
        // so, if you were in /etc and wanted to restore /etc/samba/smb.conf, httm will make certain to overwrite
        // at /etc/samba/smb.conf, not just avoid the rename
        let opt_original_live_pathdata = paths_selected_in_browse.iter().find_map(|pathdata| {
            match versions_lookup_exec(config.as_ref(), &[pathdata.clone()]).ok() {
                // safe to index into snaps, known len of 2 for set
                Some(map_live_to_snaps) => {
                    let snaps_and_live_set = map_to_snaps_live_set(&map_live_to_snaps);

                    snaps_and_live_set[0].iter().find_map(|pathdata| {
                        if pathdata == &snap_pathdata {
                            // safe to index into request, known len of 2 for set, known len of 1 for request
                            let original_live_pathdata = snaps_and_live_set[1][0].to_owned();
                            Some(original_live_pathdata)
                        } else {
                            None
                        }
                    })
                }
                None => None,
            }
        });

        match opt_original_live_pathdata {
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

    // loop until user consents or doesn't
    loop {
        let user_consent = select_restore_view(&preview_buffer, true)?.to_ascii_uppercase();

        match user_consent.as_ref() {
            "YES" | "Y" => match copy_recursive(&snap_pathdata.path_buf, &new_file_path_buf) {
                Ok(_) => {
                    let result_buffer = format!(
                        "httm copied a file from a ZFS snapshot:\n\n\
                            \tfrom: {:?}\n\
                            \tto:   {:?}\n\n\
                            Restore completed successfully.",
                        snap_pathdata.path_buf, new_file_path_buf
                    );
                    break eprintln!("{}", result_buffer);
                }
                Err(err) => {
                    return Err(HttmError::with_context(
                        "httm restore failed for the following reason",
                        Box::new(err),
                    )
                    .into());
                }
            },
            "NO" | "N" => break eprintln!("User declined restore.  No files were restored."),
            // if not yes or no, then noop and continue to the next iter of loop
            _ => {}
        }
    }

    std::process::exit(0)
}
