use crate::Config;
use crate::PathData;

use chrono::{DateTime, Local};
use number_prefix::NumberPrefix;
use std::time::SystemTime;
use lscolors::Style;
use lscolors::LsColors;
use std::path::Path;

pub fn display_raw(
    config: &Config,
    working_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut buffer = String::new();

    let delimiter = if config.opt_zeros { '\0' } else { '\n' };

    // so easy!
    for version in &working_set {
        for pd in version {
            let d_path = pd.path_buf.display().to_string();
            buffer += &format!("{}{}", d_path, delimiter);
        }
    }

    Ok(buffer)
}

pub fn display_pretty(
    config: &Config,
    collections_array: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut write_out_buffer = String::new();

    let (size_padding, fancy_string) = calculate_padding(&collections_array);

    // now display with all that beautiful padding
    if !config.opt_no_pretty {
        // only print one border to the top -- to buffer, not working_set_buffer
        write_out_buffer += &format!("{}\n", fancy_string);
    }

    for working_set in &collections_array {
        let mut working_set_buffer = String::new();
        
        for pd in working_set {

            let d_date = display_date(&pd.system_time);
            let d_size;
            let d_path;
            let fixed_padding;
            
            if !config.opt_no_pretty {
                d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
                fixed_padding = format!("{:<5}", " ");
                d_path = pd.path_buf.to_string_lossy().to_string();
            } else {
                d_size = format!("\t{}", display_human_size(pd));
                fixed_padding = "\t".to_owned();
                d_path = display_path(&pd.path_buf, Path::new(""));
            }

            if !pd.is_phantom {
                working_set_buffer +=
                    &format!("{}{}{}\"{}\"\n", d_date, d_size, fixed_padding, d_path);
            } else {
                let mut pad_date: String = String::new();
                let mut pad_size: String = String::new();
                // displays blanks for phantom values, equaling their dummy lens and dates
                // see struct PathData for more details
                //
                // again must be a better way to print padding, etc.
                for _ in 0..d_date.len() {
                    pad_date += " ";
                }
                for _ in 0..d_size.len() {
                    pad_size += " ";
                }
                working_set_buffer +=
                    &format!("{}{}{}\"{}\"\n", pad_date, pad_size, fixed_padding, d_path);
            }
        }
        if !config.opt_no_pretty && !working_set_buffer.is_empty() {
            working_set_buffer += &format!("{}\n", fancy_string);
            write_out_buffer += &working_set_buffer.to_string();
        } else {
            for line in working_set_buffer.lines().rev() {
                write_out_buffer += &format!("{}\n", line);
            }
        }
    }

    Ok(write_out_buffer)
}

fn calculate_padding(collections_array: &[Vec<PathData>]) -> (usize, String) {
    let mut size_padding = 1usize;
    let mut fancy_border = 1usize;

    // calculate padding and borders for display later
    for version in collections_array {
        for pd in version {
            let d_date = display_date(&pd.system_time);
            let d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let fixed_padding = format!("{:<5}", " ");
            let d_path = display_path(&pd.path_buf, Path::new(""));

            let d_size_len = display_human_size(pd).len();
            let formatted_line_len =
                // 2usize is for the two single quotes we add to the path display below 
                d_date.len() + d_size.len() + fixed_padding.len() + d_path.len() + 2usize;

            size_padding = d_size_len.max(size_padding);
            fancy_border = formatted_line_len.max(fancy_border);
        }
    }

    size_padding += 5usize;
    fancy_border += 5usize;

    // has to be a more idiomatic way to do this
    // if you know, let me know
    let fancy_string: String = {
        let mut res: String = String::new();
        for _ in 0..fancy_border {
            res += "-";
        }
        res
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

pub fn display_path(path: &Path, can_path: &Path) -> String {

    let lscolors = LsColors::from_env().unwrap_or_default();

    let stripped_str = if can_path == Path::new("") {
        path.to_string_lossy()
    } else {
        if let Ok(stripped_path) = &path.strip_prefix(&can_path) {
            stripped_path.to_string_lossy()
        } else {
            path.to_string_lossy()
        }
    };

    if let Some(style) = lscolors.style_for_path(&path) {
        let ansi_style = &Style::to_ansi_term_style(style);
        ansi_style.paint(stripped_str).to_string()
    } else {
        stripped_str.to_string()
    }   
}