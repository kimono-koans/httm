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

use crate::config::generate::{ExecMode, InteractiveMode, RestoreMode, RestoreSnapGuard};
use crate::data::paths::PathData;
use crate::interactive::select::InteractiveSelect;
use crate::interactive::view_mode::MultiSelect;
use crate::interactive::view_mode::ViewMode;
use crate::library::file_ops::Copy;
use crate::library::results::{HttmError, HttmResult};
use crate::library::snap_guard::SnapGuard;
use crate::library::utility::{date_string, DateFormat};
use crate::GLOBAL_CONFIG;

use nu_ansi_term::Color::LightYellow;
use terminal_size::Height;
use terminal_size::Width;

use std::path::{Path, PathBuf};

pub type InteractiveRestore = InteractiveSelect;

impl InteractiveRestore {
    pub fn restore(&self) -> HttmResult<()> {
        self.snap_path_strings
            .iter()
            .try_for_each(|snap_path_string| self.restore_per_path(snap_path_string))
    }

    fn restore_per_path(&self, snap_path_string: &str) -> HttmResult<()> {
        // build pathdata from selection buffer parsed string
        //
        // request is also sanity check for snap path exists below when we check
        // if snap_pathdata is_phantom below
        let snap_pathdata = PathData::from(Path::new(snap_path_string));

        // build new place to send file
        let new_file_path_buf = self.build_new_file_path(&snap_pathdata)?;

        let should_preserve = Self::should_preserve_attributes();

        // tell the user what we're up to, and get consent
        let preview_buffer = format!(
            "httm will perform a copy from snapshot:\n\n\
            \tsource:\t{:?}\n\
            \ttarget:\t{new_file_path_buf:?}\n\n\
            Before httm performs a restore, it would like your consent. Continue? (YES/NO)\n\
            ─────────────────────────────────────────────────────────────────────────────────────────\n\
            YES\n\
            NO",
            snap_pathdata.path_buf
        );

        // loop until user consents or doesn't
        loop {
            let view_mode = ViewMode::Restore;

            let selection = InteractiveSelect::view(&view_mode, &preview_buffer, MultiSelect::Off)?;

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
                    ) {
                        let snap_guard: SnapGuard =
                            SnapGuard::try_from(new_file_path_buf.as_path())?;

                        if let Err(err) = Copy::recursive(
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
                        if let Err(err) = Copy::recursive(
                            &snap_pathdata.path_buf,
                            &new_file_path_buf,
                            should_preserve,
                        ) {
                            let msg =
                                format!("httm restore failed for the following reason: {}.", err);
                            return Err(HttmError::new(&msg).into());
                        }
                    }

                    let result_buffer = format!(
                        "httm copied from snapshot:\n\n\
                            \tsource:\t{:?}\n\
                            \ttarget:\t{new_file_path_buf:?}\n\n\
                            Restore completed successfully.",
                        snap_pathdata.path_buf
                    );

                    let summary_string = LightYellow.paint(Self::summary_string());

                    break println!("{summary_string}{result_buffer}");
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

    fn build_new_file_path(&self, snap_pathdata: &PathData) -> HttmResult<PathBuf> {
        // build new place to send file
        if matches!(
            GLOBAL_CONFIG.exec_mode,
            ExecMode::Interactive(InteractiveMode::Restore(RestoreMode::Overwrite(_)))
        ) {
            // instead of just not naming the new file with extra info (date plus "httm_restored") and shoving that new file
            // into the pwd, here, we actually look for the original location of the file to make sure we overwrite it.
            // so, if you were in /etc and wanted to restore /etc/samba/smb.conf, httm will make certain to overwrite
            // at /etc/samba/smb.conf

            return self.opt_live_version(snap_pathdata);
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

        // remove leading dots
        let new_filename = snap_filename
            .strip_prefix(".")
            .unwrap_or(&snap_filename)
            .to_string()
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
