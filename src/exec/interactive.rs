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

use crate::config::generate::{
    ExecMode, InteractiveMode, PrintMode, RestoreMode, RestoreSnapGuard, SelectMode,
};
use crate::data::paths::{PathData, PathMetadata};
use crate::display_versions::wrapper::VersionsDisplayWrapper;
use crate::exec::preview::PreviewSelection;
use crate::exec::recursive::RecursiveSearch;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::SnapGuard;
use crate::library::utility::{
    copy_recursive, date_string, delimiter, print_output_buf, user_has_effective_root,
    user_has_zfs_allow_snap_priv, DateFormat, Never,
};
use crate::lookup::versions::VersionsMap;
use crate::{Config, GLOBAL_CONFIG};
use crossbeam_channel::unbounded;
use skim::prelude::*;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command as ExecProcess;
use std::thread;
use std::thread::JoinHandle;

#[derive(Debug)]
pub struct InteractiveBrowse {
    pub selected_pathdata: Vec<PathData>,
    pub opt_background_handle: Option<JoinHandle<()>>,
}

impl InteractiveBrowse {
    pub fn exec(interactive_mode: &InteractiveMode) -> HttmResult<Vec<PathData>> {
        let browse_result = Self::new()?;

        // do we return back to our main exec function to print,
        // or continue down the interactive rabbit hole?
        match interactive_mode {
            InteractiveMode::Restore(_) | InteractiveMode::Select(_) => {
                InteractiveSelect::exec(browse_result, interactive_mode)?;
                unreachable!()
            }
            // InteractiveMode::Browse executes back through fn exec() in main.rs
            InteractiveMode::Browse => Ok(browse_result.selected_pathdata),
        }
    }

    fn new() -> HttmResult<InteractiveBrowse> {
        let browse_result = match &GLOBAL_CONFIG.opt_requested_dir {
            // collect string paths from what we get from lookup_view
            Some(requested_dir) => {
                let view_mode = ViewMode::Browse;
                let browse_result = view_mode.browse(requested_dir)?;
                if browse_result.selected_pathdata.is_empty() {
                    return Err(HttmError::new(
                        "None of the selected strings could be converted to paths.",
                    )
                    .into());
                }

                browse_result
            }
            None => {
                // go to interactive_select early if user has already requested a file
                // and we are in the appropriate mode Select or Restore, see struct Config,
                // and None here is also used for LastSnap to skip browsing for a file/dir
                match GLOBAL_CONFIG.paths.get(0) {
                    Some(first_path) => {
                        let selected_file = first_path.clone();

                        Self {
                            selected_pathdata: vec![selected_file],
                            opt_background_handle: None,
                        }
                    }
                    // Config::from should never allow us to have an instance where we don't
                    // have at least one path to use
                    None => unreachable!(
            "GLOBAL_CONFIG.paths.get(0) should never be a None value in Interactive Mode"
          ),
                }
            }
        };

        Ok(browse_result)
    }
}

struct InteractiveSelect {
    snap_path_string: String,
    paths_selected_in_browse: Vec<PathData>,
}

impl InteractiveSelect {
    fn exec(
        browse_result: InteractiveBrowse,
        interactive_mode: &InteractiveMode,
    ) -> HttmResult<()> {
        // continue to interactive_restore or print and exit here?
        let select_result = Self::new(browse_result)?;

        match interactive_mode {
            // one only allow one to select one path string during select
            // but we retain paths_selected_in_browse because we may need
            // it later during restore if opt_overwrite is selected
            InteractiveMode::Restore(_) => InteractiveRestore::exec(select_result),
            InteractiveMode::Select(select_mode) => select_result.print_selection(select_mode),
            InteractiveMode::Browse => unreachable!(),
        }
    }

    fn new(browse_result: InteractiveBrowse) -> HttmResult<Self> {
        let versions_map = VersionsMap::new(&GLOBAL_CONFIG, &browse_result.selected_pathdata)?;

        // snap and live set has no snaps
        if versions_map.is_empty() {
            let paths: Vec<String> = browse_result
                .selected_pathdata
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

        let snap_path_string = if GLOBAL_CONFIG.opt_last_snap.is_some() {
            Self::last_snap(&browse_result.selected_pathdata, &versions_map)?
        } else {
            // same stuff we do at fn exec, snooze...
            let display_config = Config::from(browse_result.selected_pathdata.clone());

            let display_map = VersionsDisplayWrapper::from(&display_config, versions_map);

            let selection_buffer = display_map.to_string();

            let opt_live_version: Option<String> = browse_result
                .selected_pathdata
                .get(0)
                .map(|pathdata| pathdata.path_buf.to_string_lossy().into_owned());

            if display_map.map.values().all(|snaps| snaps.is_empty()) {
                if let Some(live_version) = opt_live_version {
                    eprintln!(
                        "WARN: Since {:?} has no snapshots available, quitting.",
                        live_version
                    );
                    print_output_buf(selection_buffer)?;
                    std::process::exit(0)
                }
            }

            // loop until user selects a valid snapshot version
            loop {
                let view_mode = &ViewMode::Select(opt_live_version.clone());
                // get the file name
                let requested_file_name = view_mode.select(&selection_buffer, false)?;
                // ... we want everything between the quotes
                let Some(first_match) = requested_file_name.get(0) else {
                    return Err(HttmError::new(
                        "Could not obtain a first match for the selected input",
                    )
                    .into());
                };

                let Some(path_string) = first_match
                    .split_once("\"")
                    .and_then(|(_lhs, rhs)| rhs.rsplit_once("\""))
                    .map(|(lhs, _rhs)| lhs)
                else {
                    return Err(
                        HttmError::new("Could not obtain valid path from selected input").into(),
                    );
                };

                // and cannot select a 'live' version or other invalid value.
                if display_map
                    .map
                    .keys()
                    .all(|live_version| path_string != live_version.path_buf.to_string_lossy())
                {
                    // return string from the loop
                    break path_string.to_string();
                }
            }
        };

        let paths_selected_in_browse = browse_result.selected_pathdata;

        if let Some(handle) = browse_result.opt_background_handle {
            let _ = handle.join();
        }

        Ok(Self {
            snap_path_string,
            paths_selected_in_browse,
        })
    }

    fn print_selection(&self, select_mode: &SelectMode) -> HttmResult<()> {
        let snap_path = Path::new(&self.snap_path_string);

        match select_mode {
            SelectMode::Path => {
                let delimiter = delimiter();
                let output_buf = match GLOBAL_CONFIG.print_mode {
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        format!("{}{delimiter}", self.snap_path_string)
                    }
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        format!("\"{}\"{delimiter}", self.snap_path_string)
                    }
                };

                print_output_buf(output_buf)?;

                std::process::exit(0)
            }
            SelectMode::Contents => {
                if !snap_path.is_file() {
                    let msg = format!("Path is not a file: {:?}", snap_path);
                    return Err(HttmError::new(&msg).into());
                }
                let mut f = std::fs::File::open(snap_path)?;
                let mut contents = String::new();
                f.read_to_string(&mut contents)?;

                print_output_buf(contents)?;

                std::process::exit(0)
            }
            SelectMode::Preview => {
                let opt_live_version: Option<String> = self
                    .paths_selected_in_browse
                    .get(0)
                    .map(|pathdata| pathdata.path_buf.to_string_lossy().into_owned());

                let view_mode = &ViewMode::Select(opt_live_version.clone());

                let preview_selection = PreviewSelection::new(view_mode)?;

                let cmd = if let Some(command) = preview_selection.opt_preview_command {
                    command.replace("$snap_file", &format!("{:?}", snap_path))
                } else {
                    return Err(HttmError::new("Could not parse preview command").into());
                };

                let env_command =
                    which::which("env").unwrap_or_else(|_| PathBuf::from("/usr/bin/env"));

                let spawned = ExecProcess::new(env_command)
                    .arg("bash")
                    .arg("-c")
                    .arg(cmd)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()?;

                if let Some(mut stderr) = spawned.stderr {
                    let mut output_buf = String::new();
                    stderr.read_to_string(&mut output_buf)?;
                    if !output_buf.is_empty() {
                        eprintln!("{}", &output_buf)
                    }
                }

                match spawned.stdout {
                    Some(mut stdout) => {
                        let mut output_buf = String::new();
                        stdout.read_to_string(&mut output_buf)?;
                        print_output_buf(output_buf)?;
                    }
                    None => {
                        let msg =
                            format!("Preview command output was empty for path: {:?}", snap_path);
                        return Err(HttmError::new(&msg).into());
                    }
                }

                std::process::exit(0);
            }
        }
    }

    fn last_snap(
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
                if GLOBAL_CONFIG.opt_omit_ditto {
                    snap_version.md_infallible().modify_time
                        != live_version.md_infallible().modify_time
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
    fn exec(select_result: InteractiveSelect) -> HttmResult<()> {
        // build pathdata from selection buffer parsed string
        //
        // request is also sanity check for snap path exists below when we check
        // if snap_pathdata is_phantom below
        let snap_pathdata = PathData::from(Path::new(&select_result.snap_path_string));

        // sanity check -- snap version has good metadata?
        let snap_path_metadata = snap_pathdata
            .metadata
            .ok_or_else(|| HttmError::new("Source location does not exist on disk. Quitting."))?;

        // build new place to send file
        let new_file_path_buf = Self::build_new_file_path(
            &select_result.paths_selected_in_browse,
            &snap_pathdata,
            &snap_path_metadata,
        )?;

        let should_preserve = Self::should_preserve_attributes();

        // tell the user what we're up to, and get consent
        let preview_buffer = format!(
            "httm will copy a file from a snapshot:\n\n\
            \tfrom: {:?}\n\
            \tto:   {new_file_path_buf:?}\n\n\
            Before httm restores this file, it would like your consent. Continue? (YES/NO)\n\
            ──────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO",
            snap_pathdata.path_buf
        );

        // loop until user consents or doesn't
        loop {
            let view_mode = &ViewMode::Restore;

            let selection = view_mode.select(&preview_buffer, false)?;

            let Some(user_consent) = selection.get(0) else {
                return Err(HttmError::new("Could not obtain the first match selected.").into());
            };

            match user_consent.to_ascii_uppercase().as_ref() {
                "YES" | "Y" => {
                    if matches!(
                        GLOBAL_CONFIG.exec_mode,
                        ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::Overwrite(
                            RestoreSnapGuard::Guarded
                        )))
                    ) && (user_has_effective_root().is_ok()
                        || user_has_zfs_allow_snap_priv(&new_file_path_buf).is_ok())
                    {
                        let snap_guard: SnapGuard =
                            SnapGuard::try_from(new_file_path_buf.as_path())?;

                        if let Err(err) = copy_recursive(
                            &snap_pathdata.path_buf,
                            &new_file_path_buf,
                            should_preserve,
                        ) {
                            let msg = format!(
                                "httm restore failed for the following reason: {}.\n\
                            Attempting roll back to precautionary pre-execution snapshot.",
                                err
                            );

                            eprintln!("{}", msg);

                            snap_guard
                                .rollback()
                                .map(|_| println!("Rollback succeeded."))?;

                            std::process::exit(1);
                        }
                    } else {
                        copy_recursive(
                            &snap_pathdata.path_buf,
                            &new_file_path_buf,
                            should_preserve,
                        )?
                    }

                    let result_buffer = format!(
                        "httm copied a file from a snapshot:\n\n\
                            \tfrom: {:?}\n\
                            \tto:   {new_file_path_buf:?}\n\n\
                            Restore completed successfully.",
                        snap_pathdata.path_buf
                    );

                    break println!("{result_buffer}");
                }
                "NO" | "N" => break println!("User declined restore.  No files were restored."),
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        std::process::exit(0)
    }

    fn should_preserve_attributes() -> bool {
        matches!(
            GLOBAL_CONFIG.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(
                RestoreMode::CopyAndPreserve | RestoreMode::Overwrite(_)
            ))
        )
    }

    fn build_new_file_path(
        paths_selected_in_browse: &[PathData],
        snap_pathdata: &PathData,
        snap_path_metadata: &PathMetadata,
    ) -> HttmResult<PathBuf> {
        // build new place to send file
        if matches!(
            GLOBAL_CONFIG.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::Overwrite(_)))
        ) {
            // instead of just not naming the new file with extra info (date plus "httm_restored") and shoving that new file
            // into the pwd, here, we actually look for the original location of the file to make sure we overwrite it.
            // so, if you were in /etc and wanted to restore /etc/samba/smb.conf, httm will make certain to overwrite
            // at /etc/samba/smb.conf
            let opt_original_live_pathdata = paths_selected_in_browse.iter().find_map(|pathdata| {
                match VersionsMap::new(&GLOBAL_CONFIG, &[pathdata.clone()]).ok() {
                    // safe to index into snaps, known len of 2 for set
                    Some(versions_map) => {
                        versions_map.values().flatten().find_map(|pathdata| {
                            if pathdata == snap_pathdata {
                                // SAFETY: safe to index into request, known len of 2 for set,
                                // keys and values, known len of 1 for request
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
                Some(pathdata) => Ok(pathdata.path_buf),
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
                + &date_string(
                    GLOBAL_CONFIG.requested_utc_offset,
                    &snap_path_metadata.modify_time,
                    DateFormat::Timestamp,
                );
            let new_file_dir = GLOBAL_CONFIG.pwd.as_path();
            let new_file_path_buf: PathBuf = new_file_dir.join(new_filename);

            // don't let the user rewrite one restore over another in non-overwrite mode
            if new_file_path_buf.exists() {
                Err(
                    HttmError::new("httm will not restore to that file, as a file with the same path name already exists. Quitting.").into(),
                )
            } else {
                Ok(new_file_path_buf)
            }
        }
    }
}

pub enum ViewMode {
    Browse,
    Select(Option<String>),
    Restore,
    Prune,
}

impl ViewMode {
    fn print_header(&self) -> String {
        format!(
            "PREVIEW UP: shift+up | PREVIEW DOWN: shift+down | {}\n\
        PAGE UP:    page up  | PAGE DOWN:    page down \n\
        EXIT:       esc      | SELECT:       enter      | SELECT, MULTIPLE: shift+tab\n\
        ──────────────────────────────────────────────────────────────────────────────",
            self.print_mode()
        )
    }

    fn print_mode(&self) -> &str {
        match self {
            ViewMode::Browse => "====> [ Browse Mode ] <====",
            ViewMode::Select(_) => "====> [ Select Mode ] <====",
            ViewMode::Restore => "====> [ Restore Mode ] <====",
            ViewMode::Prune => "====> [ Prune Mode ] <====",
        }
    }

    fn browse(&self, requested_dir: &Path) -> HttmResult<InteractiveBrowse> {
        // prep thread spawn
        let requested_dir_clone = requested_dir.to_path_buf();
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
        let (hangup_tx, hangup_rx): (Sender<Never>, Receiver<Never>) = bounded(0);

        // thread spawn fn enumerate_directory - permits recursion into dirs without blocking
        let background_handle = thread::spawn(move || {
            // no way to propagate error from closure so exit and explain error here
            RecursiveSearch::exec(&requested_dir_clone, tx_item.clone(), hangup_rx.clone());
        });

        let header: String = self.print_header();

        let display_handle = thread::spawn(move || {
            let opt_multi =
                GLOBAL_CONFIG.opt_last_snap.is_none() || GLOBAL_CONFIG.opt_preview.is_none();

            // create the skim component for previews
            let skim_opts = SkimOptionsBuilder::default()
                .preview_window(Some("up:50%"))
                .preview(Some(""))
                .nosort(true)
                .exact(GLOBAL_CONFIG.opt_exact)
                .header(Some(&header))
                .multi(opt_multi)
                .regex(false)
                .build()
                .expect("Could not initialized skim options for browse_view");

            // run_with() reads and shows items from the thread stream created above
            let res = match skim::Skim::run_with(&skim_opts, Some(rx_item)) {
                Some(output) if output.is_abort => {
                    eprintln!("httm interactive file browse session was aborted.  Quitting.");
                    std::process::exit(0)
                }
                Some(output) => {
                    // hangup the channel so the background recursive search can gracefully cleanup and exit
                    drop(hangup_tx);

                    output
                        .selected_items
                        .iter()
                        .map(|i| PathData::from(Path::new(&i.output().to_string())))
                        .collect()
                }
                None => {
                    return Err(HttmError::new(
                        "httm interactive file browse session failed.",
                    ));
                }
            };

            Ok(res)
        });

        match display_handle.join() {
            Ok(selected_pathdata) => {
                Self::malloc_trim();

                let res = InteractiveBrowse {
                    selected_pathdata: selected_pathdata?,
                    opt_background_handle: Some(background_handle),
                };
                Ok(res)
            }
            Err(_) => Err(HttmError::new("Interactive browse thread panicked.").into()),
        }
    }

    fn malloc_trim() {
        #[cfg(target_os = "linux")]
        #[cfg(target_env = "gnu")]
        unsafe {
            let _ = libc::malloc_trim(0);
        };
    }

    pub fn select(&self, preview_buffer: &str, opt_multi: bool) -> HttmResult<Vec<String>> {
        let preview_selection = PreviewSelection::new(self)?;

        let header = self.print_header();

        // build our browse view - less to do than before - no previews, looking through one 'lil buffer
        let skim_opts = SkimOptionsBuilder::default()
            .preview_window(preview_selection.opt_preview_window.as_deref())
            .preview(preview_selection.opt_preview_command.as_deref())
            .disabled(true)
            .tac(true)
            .nosort(true)
            .tabstop(Some("4"))
            .exact(true)
            .multi(opt_multi)
            .regex(false)
            .tiebreak(Some("length,index".to_string()))
            .header(Some(&header))
            .build()
            .expect("Could not initialized skim options for select_restore_view");

        let item_reader_opts = SkimItemReaderOption::default().ansi(true);
        let item_reader = SkimItemReader::new(item_reader_opts);

        let (items, _opt_handle) =
            item_reader.of_bufread(Box::new(Cursor::new(preview_buffer.trim().to_owned())));

        // run_with() reads and shows items from the thread stream created above
        let res = match skim::Skim::run_with(&skim_opts, Some(items)) {
            Some(output) if output.is_abort => {
                eprintln!("httm select/restore/prune session was aborted.  Quitting.");
                std::process::exit(0);
            }
            Some(output) => output
                .selected_items
                .iter()
                .map(|i| i.output().into_owned())
                .collect(),
            None => {
                return Err(HttmError::new("httm select/restore/prune session failed.").into());
            }
        };

        if GLOBAL_CONFIG.opt_debug {
            if let Some(preview_command) = preview_selection.opt_preview_command.as_deref() {
                eprintln!("DEBUG: Preview command executed: {}", preview_command)
            }
        }

        Ok(res)
    }
}
