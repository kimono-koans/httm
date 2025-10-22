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

use crate::GLOBAL_CONFIG;
use crate::config::generate::{ExecMode, InteractiveMode, RestoreMode, RestoreSnapGuard};
use crate::data::paths::{PathData, PathDeconstruction, ZfsSnapPathGuard};
use crate::interactive::select::InteractiveSelect;
use crate::interactive::view_mode::{MultiSelect, ViewMode};
use crate::library::file_ops::Copy;
use crate::library::results::{HttmError, HttmResult};
use crate::library::utility::{DateFormat, date_string, make_tmp_path};
use crate::zfs::snap_guard::SnapGuard;
use nu_ansi_term::Color::{Blue, LightYellow};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use terminal_size::{Height, Width};

pub struct InteractiveRestore {
    view_mode: ViewMode,
    snap_path_strings: Vec<String>,
    opt_live_version: Option<String>,
}

impl InteractiveRestore {
    pub fn new(interactive_select: InteractiveSelect) -> Self {
        let mut res: InteractiveRestore = unsafe { std::mem::transmute(interactive_select) };

        res.view_mode = ViewMode::Restore;

        res
    }

    pub fn restore(&self) -> HttmResult<()> {
        self.snap_path_strings
            .iter()
            .try_for_each(|snap_path_string| self.restore_per_path(snap_path_string))
    }

    fn restore_per_path(&self, snap_path_string: &str) -> HttmResult<()> {
        // build path_data from selection buffer parsed string
        //
        // request is also sanity check for snap path exists below when we check
        // if snap_path_data is_phantom below
        let snap_path_data = PathData::from(Path::new(snap_path_string));

        // build new place to send file
        let new_file_path_buf = self.build_new_file_path(&snap_path_data)?;

        let should_preserve = Self::should_preserve_attributes();

        // tell the user what we're up to, and get consent
        let restore_buffer = format!(
            "httm will perform a copy from snapshot:\n\n\
            \tsource:\t{:?}\n\
            \ttarget:\t{new_file_path_buf:?}\n\n\
            Before httm performs a restore, it would like your consent. Continue? (YES/NO)\n\
            ─────────────────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO\n",
            snap_path_data.path()
        );

        // loop until user consents or doesn't
        loop {
            let selection = self
                .view_mode
                .view_buffer(&restore_buffer, MultiSelect::Off)?;

            let user_consent = selection
                .get(0)
                .ok_or_else(|| HttmError::new("Could not obtain the first match selected."))?;

            match user_consent.to_ascii_uppercase().as_ref() {
                "YES" | "Y" => {
                    match GLOBAL_CONFIG.exec_mode {
                        ExecMode::Interactive(InteractiveMode::Restore(
                            RestoreMode::Overwrite(RestoreSnapGuard::Guarded),
                        )) => {
                            let snap_guard: SnapGuard =
                                SnapGuard::try_from(new_file_path_buf.as_ref())?;

                            if let Err(err) = Self::restore_action(
                                &snap_path_data.path(),
                                &new_file_path_buf.as_ref(),
                                Some(&snap_guard),
                                should_preserve,
                            ) {
                                eprintln!("{}", err);

                                eprintln!("Attempting rollback to snapshot guard.");

                                snap_guard
                                    .rollback()
                                    .map(|_| println!("Rollback succeeded."))?;

                                std::process::exit(1);
                            }
                        }
                        _ => Self::restore_action(
                            &snap_path_data.path(),
                            &new_file_path_buf.as_ref(),
                            None,
                            should_preserve,
                        )?,
                    }

                    let result_buffer = format!(
                        "httm copied from snapshot:\n\n\
                            \tsource:\t{:?}\n\
                            \ttarget:\t{new_file_path_buf:?}\n\n\
                            Restore completed successfully.",
                        snap_path_data.path()
                    );

                    let summary_string = LightYellow.paint(Self::summary_string());

                    break println!("{summary_string}{result_buffer}");
                }
                "NO" | "N" => {
                    break println!("User declined restore of: {:?}", snap_path_data.path());
                }
                // if not yes or no, then noop and continue to the next iter of loop
                _ => {}
            }
        }

        Ok(())
    }

    fn restore_action(
        src: &Path,
        dst: &Path,
        guarded: Option<&SnapGuard>,
        should_preserve: bool,
    ) -> HttmResult<()> {
        let copy_res = match guarded {
            Some(_) => Copy::recursive_quiet(src, dst, should_preserve),
            None => {
                let dst_tmp_path: PathBuf = make_tmp_path(&dst);

                Copy::atomic_swap(src, dst, &dst_tmp_path, should_preserve)
            }
        };

        if let Err(err) = copy_res {
            match err.downcast_ref::<std::io::Error>().map(|err| err.kind()) {
                Some(ErrorKind::PermissionDenied) => {
                    let description = format!(
                        "httm restore failed because user lacks permission to restore to the following location: {:?}.",
                        dst
                    );

                    return HttmError::from(description).into();
                }
                _ => {
                    let description = format!("httm restore failed for the following reason:");
                    return HttmError::with_source(&description, err.as_ref()).into();
                }
            };
        }

        eprintln!("{}: {:?} -> {:?}", Blue.paint("Restored "), src, dst);

        Ok(())
    }

    fn summary_string() -> String {
        let width = match terminal_size::terminal_size() {
            Some((Width(width), Height(_height))) => width as usize,
            None => 80usize,
        };

        format!("{:^width$}\n", "====> [ httm recovery summary ] <====")
    }

    fn should_preserve_attributes() -> bool {
        matches!(
            GLOBAL_CONFIG.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(
                RestoreMode::CopyAndPreserve | RestoreMode::Overwrite(_)
            ))
        )
    }

    pub fn opt_live_version(&self, snap_path_data: &PathData) -> HttmResult<Box<Path>> {
        match &self.opt_live_version {
            Some(live_version) => Some(PathBuf::from(live_version).into_boxed_path()),
            None => {
                ZfsSnapPathGuard::new(snap_path_data).and_then(|snap_guard| snap_guard.live_path())
            }
        }
        .ok_or_else(|| HttmError::new("Could not determine a possible live version.").into())
    }

    fn build_new_file_path(&self, snap_path_data: &PathData) -> HttmResult<Box<Path>> {
        // build new place to send file
        if matches!(
            GLOBAL_CONFIG.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::Overwrite(_)))
        ) {
            // instead of just not naming the new file with extra info (date plus "httm_restored") and shoving that new file
            // into the pwd, here, we actually look for the original location of the file to make sure we overwrite it.
            // so, if you were in /etc and wanted to restore /etc/samba/smb.conf, httm will make certain to overwrite
            // at /etc/samba/smb.conf

            return self.opt_live_version(snap_path_data);
        }

        let snap_filename = snap_path_data
            .path()
            .file_name()
            .expect("Could not obtain a file name for the snap file version of path given")
            .to_string_lossy()
            .into_owned();

        let Some(snap_metadata) = snap_path_data.opt_path_metadata() else {
            let description = format!(
                "Source location: {:?} does not exist on disk Quitting.",
                snap_path_data.path()
            );
            return HttmError::from(description).into();
        };

        // remove leading dots
        let new_filename = snap_filename
            .strip_prefix(".")
            .unwrap_or(&snap_filename)
            .to_string()
            + ".httm_restored."
            + &date_string(
                GLOBAL_CONFIG.requested_utc_offset,
                &snap_metadata.mtime(),
                DateFormat::Timestamp,
            );
        let new_file_dir = GLOBAL_CONFIG.pwd.as_ref();
        let new_file_path_buf: PathBuf = new_file_dir.join(new_filename);

        // don't let the user rewrite one restore over another in non-overwrite mode
        if new_file_path_buf.exists() {
            Err(
                    HttmError::new("httm will not restore to that file location, as a file with the same path name already exists. Quitting.").into(),
                )
        } else {
            Ok(new_file_path_buf.into_boxed_path())
        }
    }
}
