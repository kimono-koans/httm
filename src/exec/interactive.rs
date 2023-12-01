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
use crate::data::paths::{PathData, SnapPathGuard};
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
    snap_path_strings: Vec<String>,
    paths_selected_in_browse: Vec<PathData>,
    opt_live_version: Option<String>,
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
            InteractiveMode::Restore(_) => InteractiveRestore::exec(select_result)?,
            InteractiveMode::Select(select_mode) => select_result.print_selections(select_mode)?,
            InteractiveMode::Browse => unreachable!(),
        }

        std::process::exit(0);
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

        let opt_live_version: Option<String> = if browse_result.selected_pathdata.len() > 1 {
            None
        } else {
            browse_result
                .selected_pathdata
                .get(0)
                .map(|pathdata| pathdata.path_buf.to_string_lossy().into_owned())
        };

        let snap_path_strings = if GLOBAL_CONFIG.opt_last_snap.is_some() {
            Self::last_snaps(&browse_result.selected_pathdata, &versions_map)?
        } else {
            // same stuff we do at fn exec, snooze...
            let display_config = Config::from(browse_result.selected_pathdata.clone());

            let display_map = VersionsDisplayWrapper::from(&display_config, versions_map);

            let selection_buffer = display_map.to_string();

            display_map.map.iter().for_each(|(live, snaps)| {
                if snaps.is_empty() {
                    eprintln!("WARN: Path {:?} has no snapshots available.", live)
                }
            });

            let view_mode = ViewMode::Select(opt_live_version.clone());

            // loop until user selects a valid snapshot version
            loop {
                // get the file name
                let requested_file_names =
                    view_mode.select(&selection_buffer, InteractiveMultiSelect::On)?;

                let res = requested_file_names
                    .iter()
                    .filter_map(|selection| {
                        // ... we want everything between the quotes
                        selection
                            .split_once("\"")
                            .and_then(|(_lhs, rhs)| rhs.rsplit_once("\""))
                            .map(|(lhs, _rhs)| lhs)
                    })
                    .filter(|selection_buffer| {
                        // and cannot select a 'live' version or other invalid value.
                        display_map
                            .keys()
                            .all(|key| key.path_buf.as_path() != Path::new(selection_buffer))
                    })
                    .map(|selection_buffer| selection_buffer.to_string())
                    .collect::<Vec<String>>();

                if res.is_empty() {
                    continue;
                }

                break res;
            }
        };

        let paths_selected_in_browse = browse_result.selected_pathdata;

        if let Some(handle) = browse_result.opt_background_handle {
            let _ = handle.join();
        }

        Ok(Self {
            snap_path_strings,
            paths_selected_in_browse,
            opt_live_version,
        })
    }

    fn print_selections(&self, select_mode: &SelectMode) -> HttmResult<()> {
        self.snap_path_strings
            .iter()
            .map(Path::new)
            .try_for_each(|snap_path| self.print_snap_path(snap_path, select_mode))?;

        Ok(())
    }

    fn print_snap_path(&self, snap_path: &Path, select_mode: &SelectMode) -> HttmResult<()> {
        match select_mode {
            SelectMode::Path => {
                let delimiter = delimiter();
                let output_buf = match GLOBAL_CONFIG.print_mode {
                    PrintMode::RawNewline | PrintMode::RawZero => {
                        format!("{}{delimiter}", snap_path.to_string_lossy())
                    }
                    PrintMode::FormattedDefault | PrintMode::FormattedNotPretty => {
                        format!("\"{}\"{delimiter}", snap_path.to_string_lossy())
                    }
                };

                print_output_buf(&output_buf)?;

                Ok(())
            }
            SelectMode::Contents => {
                if !snap_path.is_file() {
                    let msg = format!("Path is not a file: {:?}", snap_path);
                    return Err(HttmError::new(&msg).into());
                }
                let mut f = std::fs::File::open(snap_path)?;
                let mut contents = Vec::new();
                f.read_to_end(&mut contents)?;

                // SAFETY: Panic here is not the end of the world as we are just printing the bytes.
                // This is the same as simply `cat`-ing the file.
                let output_buf = unsafe { std::str::from_utf8_unchecked(&contents) };

                print_output_buf(output_buf)?;

                Ok(())
            }
            SelectMode::Preview => {
                let view_mode = ViewMode::Select(self.opt_live_version.clone());

                let preview_selection = PreviewSelection::new(&view_mode)?;

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
                        print_output_buf(&output_buf)
                    }
                    None => {
                        let msg =
                            format!("Preview command output was empty for path: {:?}", snap_path);
                        Err(HttmError::new(&msg).into())
                    }
                }
            }
        }
    }

    fn last_snaps(
        paths_selected_in_browse: &[PathData],
        versions_map: &VersionsMap,
    ) -> HttmResult<Vec<String>> {
        // should be good to index into both, there is a known known 2nd vec,
        let last_snaps: Vec<String> = paths_selected_in_browse
            .iter()
            .filter_map(|live_version| {
                versions_map.get(live_version).and_then(|values| {
                    values
                        .iter()
                        .filter(|snap_version| {
                            if GLOBAL_CONFIG.opt_omit_ditto {
                                snap_version.md_infallible().modify_time
                                    != live_version.md_infallible().modify_time
                            } else {
                                true
                            }
                        })
                        .last()
                })
            })
            .map(|snap| snap.path_buf.to_string_lossy().into_owned())
            .collect();

        Ok(last_snaps)
    }
}

struct InteractiveRestore;

impl InteractiveRestore {
    fn exec(select_result: InteractiveSelect) -> HttmResult<()> {
        select_result
            .snap_path_strings
            .iter()
            .try_for_each(|snap_path_string| {
                Self::restore(snap_path_string, &select_result.paths_selected_in_browse)
            })?;

        std::process::exit(0)
    }

    fn restore(snap_path_string: &str, paths_selected_in_browse: &Vec<PathData>) -> HttmResult<()> {
        // build pathdata from selection buffer parsed string
        //
        // request is also sanity check for snap path exists below when we check
        // if snap_pathdata is_phantom below
        let snap_pathdata = PathData::from(Path::new(snap_path_string));

        // build new place to send file
        let new_file_path_buf =
            Self::build_new_file_path(&snap_pathdata, paths_selected_in_browse)?;

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

            let selection = view_mode.select(&preview_buffer, InteractiveMultiSelect::Off)?;

            let user_consent = selection
                .get(0)
                .ok_or_else(|| HttmError::new("Could not obtain the first match selected."))?;

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
                "NO" | "N" => {
                    break println!("User declined restore of: {:?}", snap_pathdata.path_buf)
                }
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        Ok(())
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
        snap_pathdata: &PathData,
        paths_selected_in_browse: &Vec<PathData>,
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

            return opt_live_version(snap_pathdata, paths_selected_in_browse);
        }

        let snap_filename = snap_pathdata
            .path_buf
            .file_name()
            .expect("Could not obtain a file name for the snap file version of path given")
            .to_string_lossy()
            .into_owned();

        let Some(snap_metadata) = snap_pathdata.metadata else {
            let msg = format!(
                "Source location: {:?} does not exist on disk Quitting.",
                snap_pathdata.path_buf
            );
            return Err(HttmError::new(&msg).into());
        };

        let new_filename = snap_filename
            + ".httm_restored."
            + &date_string(
                GLOBAL_CONFIG.requested_utc_offset,
                &snap_metadata.modify_time,
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

pub enum ViewMode {
    Browse,
    Select(Option<String>),
    Restore,
    Prune,
}

pub enum InteractiveMultiSelect {
    On,
    Off,
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
            let opt_multi = GLOBAL_CONFIG.opt_preview.is_none();

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
                #[cfg(feature = "linux_malloc_trim")]
                #[cfg(target_os = "linux")]
                #[cfg(target_env = "gnu")]
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

    #[cfg(feature = "linux_malloc_trim")]
    #[cfg(target_os = "linux")]
    #[cfg(target_env = "gnu")]
    fn malloc_trim() {
        unsafe {
            let _ = libc::malloc_trim(0);
        };
    }

    pub fn select(
        &self,
        preview_buffer: &str,
        opt_multi: InteractiveMultiSelect,
    ) -> HttmResult<Vec<String>> {
        let preview_selection = PreviewSelection::new(self)?;

        let header = self.print_header();

        let opt_multi = match opt_multi {
            InteractiveMultiSelect::On => true,
            InteractiveMultiSelect::Off => false,
        };

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

        let (items, opt_ingest_handle) =
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

        if let Some(handle) = opt_ingest_handle {
            let _ = handle.join();
        };

        if GLOBAL_CONFIG.opt_debug {
            if let Some(preview_command) = preview_selection.opt_preview_command.as_deref() {
                eprintln!("DEBUG: Preview command executed: {}", preview_command)
            }
        }

        Ok(res)
    }
}

pub fn opt_live_version(
    snap_pathdata: &PathData,
    paths_selected_in_browse: &Vec<PathData>,
) -> HttmResult<PathBuf> {
    match SnapPathGuard::new(snap_pathdata) {
        Some(original_live_pathdata) => return Ok(original_live_pathdata.path_buf.clone()),
        None => {
            return paths_selected_in_browse
                .iter()
                .max_by_key(|live_path| {
                    snap_pathdata
                        .path_buf
                        .ancestors()
                        .zip(live_path.path_buf.ancestors())
                        .take_while(|(a_path, b_path)| a_path == b_path)
                        .count()
                })
                .map(|pd| pd.path_buf.clone())
                .ok_or_else(|| {
                    HttmError::new(
                        "httm unable to determine original file path in overwrite mode.  Quitting.",
                    )
                    .into()
                });
        }
    }
}
