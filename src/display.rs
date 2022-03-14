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

use crate::Config;
use crate::PathData;

use chrono::{DateTime, Local};
use lscolors::LsColors;
use lscolors::Style;
use number_prefix::NumberPrefix;
use std::borrow::Cow;
use std::path::Path;
use std::time::SystemTime;
use terminal_size::{terminal_size, Height, Width};

pub fn display_raw(
    config: &Config,
    snaps_and_live_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut buffer = String::new();

    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    // so easy!
    for version in &snaps_and_live_set {
        for pd in version {
            let display_path = pd.path_buf.display().to_string();
            buffer += &format!("{}{}", display_path, delimiter);
        }
    }

    Ok(buffer)
}

pub fn display_pretty(
    config: &Config,
    snaps_and_live_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut write_out_buffer = String::new();

    let (size_padding, fancy_string) = calculate_padding(&snaps_and_live_set);

    // now display with all that beautiful padding
    if !config.opt_no_pretty {
        // only print one border to the top -- to buffer, not pathdata_set_buffer
        write_out_buffer += &format!("{}\n", fancy_string);
    }

    for pathdata_set in &snaps_and_live_set {
        let mut pathdata_set_buffer = String::new();

        for pathdata in pathdata_set {
            let display_date = display_date(&pathdata.system_time);
            let display_size;
            let fixed_padding;
            let display_path = &pathdata.path_buf.to_string_lossy();

            if !config.opt_no_pretty {
                display_size = format!("{:>width$}", display_human_size(pathdata), width = size_padding);
                fixed_padding = format!("{:<5}", " ");
            } else {
                display_size = format!("\t{}", display_human_size(pathdata));
                fixed_padding = "\t".to_owned();
            }

            if !pathdata.is_phantom {
                pathdata_set_buffer += &format!("{}{}{}\"{}\"\n", display_date, display_size, fixed_padding, display_path);
            } else {
                // displays blanks for phantom values, equaling their dummy lens and dates
                // see struct PathData for more details
                //
                // again must be a better way to print padding, etc.
                let pad_date: String = (0..display_date.len()).map(|_| " ").collect();
                let pad_size: String = (0..display_size.len()).map(|_| " ").collect();
                pathdata_set_buffer +=
                    &format!("{}{}{}\"{}\"\n", pad_date, pad_size, fixed_padding, display_path);
            }
        }
        if !config.opt_no_pretty && !pathdata_set_buffer.is_empty() {
            pathdata_set_buffer += &format!("{}\n", fancy_string);
            write_out_buffer += &pathdata_set_buffer.to_string();
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
        for pd in ver_set {
            let display_date = display_date(&pd.system_time);
            let display_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let fixed_padding = format!("{:<5}", " ");
            let display_path = &pd.path_buf.to_string_lossy();

            let display_size_len = display_human_size(pd).len();
            let formatted_line_len =
                // 2usize is for the two single quotes we add to the path display below 
                display_date.len() + display_size.len() + fixed_padding.len() + display_path.len() + 2usize;

            size_padding = display_size_len.max(size_padding);
            fancy_border = formatted_line_len.max(fancy_border);
        }
    }

    size_padding += 5usize;
    fancy_border += 5usize;

    // has to be a more idiomatic way to do this
    // if you know, let me know

    let fancy_string: String = if let Some((Width(w), Height(_h))) = terminal_size() {
        if (w as usize) < fancy_border {
            (0..w as usize).map(|_| "─").collect()
        } else {
            (0..fancy_border).map(|_| "─").collect()
        }
    } else {
        (0..fancy_border).map(|_| "─").collect()
    };

    (size_padding, fancy_string)
}

fn display_human_size(pd: &PathData) -> String {
    let size = pd.size as f64;

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
    format!("{}", dt.format("%b %e %H:%M:%S %Y"))
}

pub fn display_colors(stripped_str: Cow<str>, path: &Path) -> String {
    let lscolors = LsColors::from_env().unwrap_or_default();

    if let Some(style) = lscolors.style_for_path(&path) {
        let ansi_style = &Style::to_ansi_term_style(style);
        ansi_style.paint(stripped_str).to_string()
    } else {
        stripped_str.to_string()
    }
}
