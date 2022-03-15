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

use crate::display::{display_colors, display_pretty};
use crate::lookup::run_search;
use crate::read_stdin;
use crate::Config;
use crate::HttmError;
use crate::{convert_strings_to_pathdata, InteractiveMode};

extern crate skim;
use chrono::DateTime;
use chrono::Local;
use skim::prelude::*;
use skim::DisplayContext;
use std::fs::ReadDir;
use std::io::Cursor;
use std::io::Write as IoWrite;
use std::process::Command as ExecProcess;
use std::thread;
use std::time::SystemTime;
use std::vec;

use std::io::Stdout;
use std::path::{Path, PathBuf};

fn lookup_view(config: &Config) -> Result<String, Box<dyn std::error::Error>> {
    // build our paths for the httm preview invocations, we need Strings because for now
    // skim does not allow invoking preview from a Rust function, we actually just exec httm again
    let local_dir = if let Some(local_dir) = &config.opt_local_dir {
        local_dir.to_string_lossy()
    } else {
        config.current_working_dir.to_string_lossy()
    };

    let canonical_parent = if let Ok(canonical_parent) = config.user_requested_dir.canonicalize() {
        canonical_parent
    } else {
        config.current_working_dir.clone()
    };

    // prep thread spawn
    let mut read_dir = std::fs::read_dir(&canonical_parent)?;
    let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
    let config_clone = config.clone();
    let canonical_parent_clone = canonical_parent.clone();

    // spawn recursive fn enter_directory
    thread::spawn(move || {
        enter_directory(
            &config_clone,
            &tx_item,
            &mut read_dir,
            &canonical_parent_clone,
        );
    });

    // string to exec on each preview
    let path_command =
        std::str::from_utf8(&ExecProcess::new("which").arg("httm").output()?.stdout)?.to_owned();

    // skim doesn't allow us to use a function, we must call a command
    // and that cause all sorts of nastiness with PATHs etc if the user
    // is not expecting it
    let httm_command = if path_command.is_empty() {
        let path: PathBuf = [&config.current_working_dir, &PathBuf::from("httm")]
            .iter()
            .collect();
        if path.exists() {
            path.to_string_lossy().to_string()
        } else {
            return Err(HttmError::new(
                "You must place the httm command in your path.  Perhaps the .cargo/bin folder isn't in your path?",
            )
            .into());
        }
    } else {
        path_command.trim_end_matches('\n').to_string()
    };

    // create command to use for preview, as noted unable to use a function for now
    let cp_string = canonical_parent.to_string_lossy();
    let preview_str = if let Some(sp_os) = &config.opt_snap_point {
        let snap_point = sp_os.to_string_lossy();
        format!(
            "\"{httm_command}\" --snap-point \"{snap_point}\" --local-dir \"{local_dir}\" \"{}\"/{{}}",
            cp_string
        )
    } else {
        format!("\"{httm_command}\" \"{}\"/{{}}", cp_string)
    };

    // create the skim component for previews
    let options = SkimOptionsBuilder::default()
        .preview_window(Some("70%"))
        .preview(Some(&preview_str))
        .build()
        .unwrap();

    let selected_items = Skim::run_with(&options, Some(rx_item))
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    let res = selected_items
        .iter()
        .map(|i| i.output().to_string())
        .collect();

    Ok(res)
}
struct SelectionCandidate {
    path: PathBuf,
    canonical_parent: PathBuf,
}

impl SkimItem for SelectionCandidate {
    fn text(&self) -> Cow<str> {
        Cow::Owned(path_to_string(&self.path, &self.canonical_parent))
    }
    fn display<'a>(&'a self, _context: DisplayContext<'a>) -> AnsiString<'a> {
        AnsiString::parse(&display_colors(self.text(), &self.path))
    }
}

fn path_to_string(path: &Path, canonical_parent: &Path) -> String {
    let stripped_str = if canonical_parent == Path::new("") {
        path.to_string_lossy()
    } else if let Ok(stripped_path) = &path.strip_prefix(&canonical_parent) {
        stripped_path.to_string_lossy()
    } else {
        path.to_string_lossy()
    };
    stripped_str.to_string()
}

fn enter_directory(
    config: &Config,
    tx_item: &SkimItemSender,
    read_dir: &mut ReadDir,
    canonical_parent: &Path,
) {
    // convert to paths
    let (vec_files, vec_dirs): (Vec<PathBuf>, Vec<PathBuf>) = read_dir
        .filter_map(|i| i.ok())
        .map(|dir_entry| dir_entry.path())
        .partition(|path| path.is_file() || path.is_symlink());

    // display with pretty ANSI colors
    let mut combined_vec: Vec<&PathBuf> =
        vec![&vec_files, &vec_dirs].into_iter().flatten().collect();
    combined_vec.sort();
    combined_vec.iter().for_each(|path| {
        let _ = tx_item.send(Arc::new(SelectionCandidate {
            path: path.to_path_buf(),
            canonical_parent: canonical_parent.to_path_buf(),
        }));
    });

    // now recurse, if requested
    if config.opt_recursive {
        vec_dirs
            .iter()
            .filter_map(|read_dir| std::fs::read_dir(read_dir).ok())
            .for_each(|mut read_dir| {
                enter_directory(config, tx_item, &mut read_dir, canonical_parent);
            })
    }
}

fn select_view(selection_buffer: String) -> Result<String, Box<dyn std::error::Error>> {
    let options = SkimOptionsBuilder::default()
        .interactive(true)
        .build()
        .unwrap();

    // `SkimItemReader` is a helper to turn any `BufRead` into a stream of `SkimItem`
    // `SkimItem` was implemented for `AsRef<str>` by default
    let item_reader = SkimItemReader::default();
    let items = item_reader.of_bufread(Cursor::new(selection_buffer));

    // `run_with` would read and show items from the stream
    let selected_items = Skim::run_with(&options, Some(items))
        .map(|out| out.selected_items)
        .unwrap_or_else(Vec::new);

    let res = selected_items
        .iter()
        .map(|i| i.output().to_string())
        .collect();

    Ok(res)
}

pub fn interactive_exec(
    out: &mut Stdout,
    config: &Config,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let paths_as_strings = if config.raw_paths.is_empty()
        || PathBuf::from(&config.raw_paths[0]).is_dir()
    {
        vec![lookup_view(config)?]
    } else if config.raw_paths.len().gt(&1usize) {
        return Err(HttmError::new("May only specify one path in interactive mode.").into());
    } else if !Path::new(&config.raw_paths[0]).is_dir() {
        return Err(
            HttmError::new("Path specified is not a directory suitable for browsing.").into(),
        );
    } else {
        unreachable!("Nope, nope, you shouldn't be here!!  Just kidding, file a bug if you find yourself here.");
    };

    match config.interactive_mode {
        InteractiveMode::Restore | InteractiveMode::Select => {
            interactive_select(out, config, paths_as_strings)?;
            unreachable!("You *really* shouldn't be here!!! No.... no.... AHHHHHHHHGGGGG... Just kidding, file a bug if you find yourself here.")
        },
        // InteractiveMode::Lookup executes back through fn exec() in httm.rs
        _ => Ok(paths_as_strings),
    }
}

fn interactive_select(
    out: &mut Stdout,
    config: &Config,
    paths_as_strings: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // same stuff we do at exec, snooze...
    let search_path = paths_as_strings.get(0).unwrap().to_owned();
    let pathdata_set = convert_strings_to_pathdata(config, &[search_path])?;
    let snaps_and_live_set = run_search(config, pathdata_set)?;
    let selection_buffer = display_pretty(config, snaps_and_live_set)?;

    // file name ready to do some file ops!!
    // ... we want the 2nd item or the indexed "1" object
    // everything between the quotes
    let requested_file_name = select_view(selection_buffer)?;
    let broken_string: Vec<_> = requested_file_name.split_terminator('"').collect();
    let parsed_str = if let Some(parsed) = broken_string.get(1) {
        parsed
    } else {
        return Err(HttmError::new("Invalid value selected. Quitting.").into());
    };

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
) -> Result<(), Box<dyn std::error::Error>> {
    let snap_pbuf = PathBuf::from(&parsed_str);

    let snap_md = if let Ok(snap_md) = snap_pbuf.metadata() {
        snap_md
    } else {
        return Err(HttmError::new("Snapshot location does not exist on disk. Quitting.").into());
    };

    // build new place to send file
    let old_snap_filename = snap_pbuf.file_name().unwrap().to_string_lossy().to_string();
    let new_snap_filename: String =
        old_snap_filename.clone() + ".httm_restored." + &timestamp_file(&snap_md.modified()?);

    let new_file_dir = config.current_working_dir.clone();
    let new_file_pbuf: PathBuf = [new_file_dir, PathBuf::from(new_snap_filename)]
        .iter()
        .collect();

    let old_file_dir = config.current_working_dir.clone();
    let old_file_pbuf: PathBuf = [old_file_dir, PathBuf::from(old_snap_filename)]
        .iter()
        .collect();

    if old_file_pbuf == snap_pbuf {
        return Err(
            HttmError::new("Will not restore files as files are the same file. Quitting.").into(),
        );
    };

    // tell the user what we're up to
    write!(out, "httm will copy a file from a ZFS snapshot...\n\n")?;
    writeln!(out, "\tfrom: {:?}", snap_pbuf)?;
    writeln!(out, "\tto:   {:?}\n", new_file_pbuf)?;
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
        std::fs::copy(snap_pbuf, new_file_pbuf)?;
        write!(out, "\nRestore completed successfully.\n")?;
    } else {
        write!(out, "\nUser declined.  No files were restored.\n")?;
    }

    std::process::exit(0)
}

fn timestamp_file(st: &SystemTime) -> String {
    let dt: DateTime<Local> = st.to_owned().into();
    format!("{}", dt.format("%b-%d-%H:%M:%S-%Y"))
}
