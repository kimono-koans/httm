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

use crate::{Config, PathData};

use chrono::{DateTime, Local};
use lscolors::{LsColors, Style};
use number_prefix::NumberPrefix;
use std::{path::Path, time::SystemTime};
use terminal_size::{terminal_size, Height, Width};

pub fn display_exec(
    config: &Config,
    snaps_and_live_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let output_buffer = if config.opt_raw || config.opt_zeros {
        display_raw(config, snaps_and_live_set)?
    } else {
        display_pretty(config, snaps_and_live_set)?
    };

    Ok(output_buffer)
}

fn display_raw(
    config: &Config,
    snaps_and_live_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut buffer = String::new();

    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    // so easy!
    for version in &snaps_and_live_set {
        for pathdata in version {
            let display_path = pathdata.path_buf.display();
            buffer += &format!("{}{}", display_path, delimiter);
        }
    }

    Ok(buffer)
}

fn display_pretty(
    config: &Config,
    snaps_and_live_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let mut write_out_buffer = String::new();

    let (size_padding, fancy_border_string) = calculate_padding(&snaps_and_live_set);

    // now display with all that beautiful padding
    if !config.opt_no_pretty {
        // only print one border to the top -- to write_out_buffer, not pathdata_set_buffer
        write_out_buffer += &format!("{}\n", fancy_border_string);
    }

    for (idx, pathdata_set) in snaps_and_live_set.iter().enumerate() {
        let mut pathdata_set_buffer = String::new();

        for pathdata in pathdata_set {
            let display_date = display_date(&pathdata.system_time);
            let fixed_padding: String = (0..5).map(|_| " ").collect();
            let display_size;
            let display_path;

            // paint the live string with ls colors
            let painted_path = if idx == 1 {
                let path = &pathdata.path_buf;
                paint_string(path, &path.to_string_lossy())
            } else {
                pathdata.path_buf.to_string_lossy().into_owned()
            };

            // tab delimited if "no pretty", and no border lines
            if !config.opt_no_pretty {
                display_size = format!(
                    "{:>width$}",
                    display_human_size(pathdata),
                    width = size_padding
                );
                display_path = format!("{:<width$}", painted_path, width = 5);
            } else {
                display_size = display_human_size(pathdata);
                display_path = painted_path;
            }

            // displays blanks for phantom values, equaling their dummy lens and dates.
            //
            // see struct PathData for more details as to why we use a dummy instead of
            // a None value here.
            if !pathdata.is_phantom {
                pathdata_set_buffer += &format!(
                    "{}{}{}{}\"{}\"\n",
                    display_date, fixed_padding, display_size, fixed_padding, display_path
                );
            } else {
                let phantom_date: String = (0..display_date.len()).map(|_| " ").collect();
                let phantom_size: String = (0..display_size.len()).map(|_| " ").collect();
                pathdata_set_buffer += &format!(
                    "{}{}{}{}\"{}\"\n",
                    phantom_date, fixed_padding, phantom_size, fixed_padding, display_path
                );
            }
        }
        if !config.opt_no_pretty && !pathdata_set_buffer.is_empty() {
            pathdata_set_buffer += &format!("{}\n", fancy_border_string);
            write_out_buffer += &pathdata_set_buffer;
        } else {
            for line in pathdata_set_buffer.lines().rev() {
                write_out_buffer += &format!("{}\n", line);
            }
        }
    }

    Ok(write_out_buffer)
}

fn calculate_padding(snaps_and_live_set: &[Vec<PathData>]) -> (usize, String) {
    let mut size_padding = 1usize;
    let mut fancy_border = 1usize;

    // calculate padding and borders for display later
    for ver_set in snaps_and_live_set {
        for pathdata in ver_set {
            let display_date = display_date(&pathdata.system_time);
            let display_size = format!(
                "{:>width$}",
                display_human_size(pathdata),
                width = size_padding
            );
            let fixed_padding: String = (0..5).map(|_| " ").collect();
            let display_path = &pathdata.path_buf.to_string_lossy();

            let display_size_len = display_human_size(pathdata).len();
            let formatted_line_len =
                // addition of 2usize is for the two quotation marks we add to the path display 
                display_date.len() + display_size.len() + (2 * fixed_padding.len()) + display_path.len() + 2usize;

            size_padding = display_size_len.max(size_padding);
            fancy_border = formatted_line_len.max(fancy_border);
        }
    }

    // has to be a more idiomatic way to do this
    // if you know, let me know
    let fancy_border_string: String = if let Some((Width(w), Height(_h))) = terminal_size() {
        if (w as usize) < fancy_border {
            (0..w as usize).map(|_| "─").collect()
        } else {
            (0..fancy_border).map(|_| "─").collect()
        }
    } else {
        (0..fancy_border).map(|_| "─").collect()
    };

    (size_padding, fancy_border_string)
}

fn display_human_size(pathdata: &PathData) -> String {
    let size = pathdata.size as f64;

    match NumberPrefix::binary(size) {
        NumberPrefix::Standalone(bytes) => {
            format!("{} bytes", bytes)
        }
        NumberPrefix::Prefixed(prefix, n) => {
            format!("{:.1} {}B", n, prefix)
        }
    }
}

fn display_date(st: &SystemTime) -> String {
    let dt: DateTime<Local> = st.to_owned().into();
    format!("{}", dt.format("%b %e %Y %H:%M:%S"))
}

pub fn paint_string(path: &Path, file_name: &str) -> String {
    let lscolors = LsColors::from_env().unwrap_or_default();

    if let Some(style) = lscolors.style_for_path(path) {
        let ansi_style = &Style::to_ansi_term_style(style);
        ansi_style.paint(file_name).to_string()
    } else {
        file_name.to_owned()
    }
}
