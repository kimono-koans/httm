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

use std::{io::Cursor, path::Path, path::PathBuf, thread};

use crossbeam::channel::unbounded;
use skim::prelude::*;

use crate::config::generate::{Config, ExecMode, InteractiveMode, PrintMode, RestoreMode};
use crate::data::paths::{PathData, PathMetadata};
use crate::data::selection::SelectionCandidate;
use crate::exec::display::DisplayWrapper;
use crate::exec::preview::PreviewSelection;
use crate::exec::recursive::RecursiveLoop;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{
    copy_recursive, get_date, get_delimiter, print_output_buf, DateFormat, Never,
};
use crate::lookup::versions::VersionsMap;

pub struct InteractiveBrowse;

impl InteractiveBrowse {
    pub fn exec(
        config: Arc<Config>,
        interactive_mode: &InteractiveMode,
    ) -> HttmResult<Vec<PathData>> {
        let paths_selected_in_browse = match &config.opt_requested_dir {
            // collect string paths from what we get from lookup_view
            Some(requested_dir) => {
                // loop until user selects a valid path
                loop {
                    let selected_pathdata =
                        InteractiveBrowse::browse_view(config.clone(), requested_dir)?
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
                        InteractiveSelect::exec(
                            config.as_ref(),
                            &[selected_file],
                            interactive_mode,
                        )?;
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
            InteractiveMode::Restore(_) | InteractiveMode::Select => {
                InteractiveSelect::exec(
                    config.as_ref(),
                    &paths_selected_in_browse,
                    interactive_mode,
                )?;
                unreachable!()
            }
            // InteractiveMode::Browse executes back through fn exec() in main.rs
            InteractiveMode::Browse => Ok(paths_selected_in_browse.to_vec()),
        }
    }

    #[allow(unused_variables)]
    fn browse_view(config: Arc<Config>, requested_dir: &PathData) -> HttmResult<Vec<String>> {
        // prep thread spawn
        let requested_dir_clone = requested_dir.path_buf.clone();
        let config_clone = config.clone();
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
        let (hangup_tx, hangup_rx): (Sender<Never>, Receiver<Never>) = bounded(0);

        // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
        thread::spawn(move || {
            // no way to propagate error from closure so exit and explain error here
            RecursiveLoop::exec(
                config_clone,
                &requested_dir_clone,
                tx_item.clone(),
                hangup_rx.clone(),
            )
        });

        let opt_multi = config.opt_last_snap.is_none() || config.opt_preview.is_none();

        // create the skim component for previews
        let skim_opts = SkimOptionsBuilder::default()
            .preview_window(Some("up:50%"))
            .preview(Some(""))
            .nosort(true)
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
        let selected_items = if let Some(output) = Skim::run_with(&skim_opts, Some(rx_item)) {
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
}

struct InteractiveSelect;

impl InteractiveSelect {
    fn exec(
        config: &Config,
        paths_selected_in_browse: &[PathData],
        interactive_mode: &InteractiveMode,
    ) -> HttmResult<()> {
        let versions_map = VersionsMap::new(config, paths_selected_in_browse)?;

        // snap and live set has no snaps
        if versions_map.is_empty() {
            let paths: Vec<String> = paths_selected_in_browse
                .iter()
                .map(|path| path.path_buf.to_string_lossy().to_string())
                .collect();
            let msg = format!(
                "{}{:?}",
                "Cannot select or restore from the following paths as they have no snapshots:\n",
                paths
            );
            return Err(HttmError::new(&msg).into());
        }

        let path_string = if config.opt_last_snap.is_some() {
            Self::get_last_snap(config, paths_selected_in_browse, &versions_map)?
        } else {
            // same stuff we do at fn exec, snooze...
            let display_config =
                SelectionCandidate::generate_config_for_display(config, paths_selected_in_browse);

            let display_map = DisplayWrapper::from(&display_config, versions_map);

            let selection_buffer = display_map.to_string();

            let opt_live_version: Option<String> = paths_selected_in_browse
                .as_ref()
                .get(0)
                .map(|pathdata| pathdata.path_buf.to_string_lossy().into_owned());

            // loop until user selects a valid snapshot version
            loop {
                // get the file name
                let requested_file_name = select_restore_view(
                    config,
                    &selection_buffer,
                    ViewMode::Select(opt_live_version.clone()),
                )?;
                // ... we want everything between the quotes
                let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
                // ... and the file is the 2nd item or the indexed "1" object
                if let Some(path_string) = broken_string.get(1) {
                    // and cannot select a 'live' version or other invalid value.
                    if display_map.map.iter().all(|(live_version, _snaps)| {
                        Path::new(path_string) != live_version.path_buf.as_path()
                    }) {
                        // return string from the loop
                        break path_string.to_string();
                    }
                }
            }
        };

        // continue to interactive_restore or print and exit here?
        if matches!(interactive_mode, InteractiveMode::Restore(_)) {
            // one only allow one to select one path string during select
            // but we retain paths_selected_in_browse because we may need
            // it later during restore if opt_overwrite is selected
            Ok(InteractiveRestore::exec(
                config,
                &path_string,
                paths_selected_in_browse,
            )?)
        } else {
            Ok(Self::print_selection(config, &path_string)?)
        }
    }

    fn print_selection(config: &Config, path_string: &str) -> HttmResult<()> {
        let delimiter = get_delimiter(config);

        let output_buf = if matches!(
            config.print_mode,
            PrintMode::RawNewline | PrintMode::RawZero
        ) {
            format!("{}{}", path_string, delimiter)
        } else {
            format!("\"{}\"{}", path_string, delimiter)
        };

        print_output_buf(output_buf)?;

        std::process::exit(0)
    }

    fn get_last_snap(
        config: &Config,
        paths_selected_in_browse: &[PathData],
        versions_map: &VersionsMap,
    ) -> HttmResult<String> {
        // should be good to index into both, there is a known known 2nd vec,
        let live_version = &paths_selected_in_browse
            .get(0)
            .expect("ExecMode::LiveSnap should always have exactly one path.");

        let last_snap = versions_map
            .values()
            .flatten()
            .filter(|snap_version| {
                if config.opt_omit_ditto {
                    snap_version.get_md_infallible().modify_time
                        != live_version.get_md_infallible().modify_time
                } else {
                    true
                }
            })
            .last()
            .ok_or_else(|| HttmError::new("No last snapshot for the requested input file exists."))?
            .path_buf
            .to_string_lossy()
            .into_owned();

        Ok(last_snap)
    }
}

struct InteractiveRestore;

impl InteractiveRestore {
    fn exec(
        config: &Config,
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
        let (new_file_path_buf, should_preserve) = Self::build_new_file_path(
            config,
            paths_selected_in_browse,
            &snap_pathdata,
            &snap_path_metadata,
        )?;

        // tell the user what we're up to, and get consent
        let preview_buffer = format!(
            "httm will copy a file from a snapshot:\n\n\
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
            let user_consent = select_restore_view(config, &preview_buffer, ViewMode::Restore)?
                .to_ascii_uppercase();

            match user_consent.as_ref() {
                "YES" | "Y" => {
                    copy_recursive(&snap_pathdata.path_buf, &new_file_path_buf, should_preserve)?;

                    let result_buffer = format!(
                        "httm copied a file from a snapshot:\n\n\
                            \tfrom: {:?}\n\
                            \tto:   {:?}\n\n\
                            Restore completed successfully.",
                        snap_pathdata.path_buf, new_file_path_buf
                    );

                    break eprintln!("{}", result_buffer);
                }
                "NO" | "N" => break eprintln!("User declined restore.  No files were restored."),
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        std::process::exit(0)
    }

    fn build_new_file_path(
        config: &Config,
        paths_selected_in_browse: &[PathData],
        snap_pathdata: &PathData,
        snap_path_metadata: &PathMetadata,
    ) -> HttmResult<(PathBuf, bool)> {
        // build new place to send file
        if matches!(
            config.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::Overwrite))
        ) {
            // instead of just not naming the new file with extra info (date plus "httm_restored") and shoving that new file
            // into the pwd, here, we actually look for the original location of the file to make sure we overwrite it.
            // so, if you were in /etc and wanted to restore /etc/samba/smb.conf, httm will make certain to overwrite
            // at /etc/samba/smb.conf
            let opt_original_live_pathdata = paths_selected_in_browse.iter().find_map(|pathdata| {
                match VersionsMap::new(config, &[pathdata.clone()]).ok() {
                    // safe to index into snaps, known len of 2 for set
                    Some(versions_map) => {
                        versions_map.values().flatten().find_map(|pathdata| {
                            if pathdata == snap_pathdata {
                                // safe to index into request, known len of 2 for set, keys and values, known len of 1 for request
                                let original_live_pathdata =
                                    versions_map.keys().next().unwrap().clone();
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
                Some(pathdata) => {
                    let should_preserve = true;
                    Ok((pathdata.path_buf, should_preserve))
                }
                None => Err(HttmError::new(
                    "httm unable to determine original file path in overwrite mode.  Quitting.",
                )
                .into()),
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
                    config,
                    &snap_path_metadata.modify_time,
                    DateFormat::Timestamp,
                );
            let new_file_dir = config.pwd.path_buf.clone();
            let new_file_path_buf: PathBuf = new_file_dir.join(new_filename);

            // don't let the user rewrite one restore over another in non-overwrite mode
            if new_file_path_buf.exists() {
                Err(
                    HttmError::new("httm will not restore to that file, as a file with the same path name already exists. Quitting.").into(),
                )
            } else {
                let should_preserve = matches!(
                    config.exec_mode,
                    ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::CopyAndPreserve))
                );
                Ok((new_file_path_buf, should_preserve))
            }
        }
    }
}

pub enum ViewMode {
    Select(Option<String>),
    Restore,
}

fn select_restore_view(
    config: &Config,
    preview_buffer: &str,
    view_mode: ViewMode,
) -> HttmResult<String> {
    let preview_selection = PreviewSelection::new(config, view_mode)?;

    // build our browse view - less to do than before - no previews, looking through one 'lil buffer
    let skim_opts = SkimOptionsBuilder::default()
        .preview_window(preview_selection.opt_preview_window.as_deref())
        .preview(preview_selection.opt_preview_command.as_deref())
        .disabled(true)
        .tac(true)
        .nosort(true)
        .tabstop(Some("4"))
        .exact(true)
        .multi(false)
        .regex(false)
        .tiebreak(Some("length,index".to_string()))
        .header(Some(
            "PREVIEW UP: shift+up | PREVIEW DOWN: shift+down\n\
                PAGE UP:    page up  | PAGE DOWN:    page down \n\
                EXIT:       esc      | SELECT:       enter      | SELECT, MULTIPLE: shift+tab\n\
                ──────────────────────────────────────────────────────────────────────────────",
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
