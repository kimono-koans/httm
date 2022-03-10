use crate::Config;
use crate::PathData;

use chrono::{DateTime, Local};
use number_prefix::NumberPrefix;
use std::time::SystemTime;

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
    working_set: Vec<Vec<PathData>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut size_padding = 1usize;
    let mut fancy_border = 1usize;
    let mut buffer = String::new();

    // calculate padding and borders for display later
    for version in &working_set {
        for pd in version {
            let d_date = display_date(&pd.system_time);
            let d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let fixed_padding = format!("{:<5}", " ");
            let d_path = pd.path_buf.display().to_string();

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

    let mut instance_buffer = String::new();

    // now display with all that beautiful padding
    if !config.opt_no_pretty {
        instance_buffer += &format!("{}\n", fancy_string);
    }

    for version in &working_set {
        for pd in version {
            let d_date = display_date(&pd.system_time);
            let mut d_size = format!("{:>width$}", display_human_size(pd), width = size_padding);
            let mut fixed_padding = format!("{:<5}", " ");
            let d_path = pd.path_buf.display();

            if config.opt_no_pretty {
                fixed_padding = "\t".to_owned();
                d_size = format!("\t{}", display_human_size(pd));
            };

            if !pd.is_phantom {
                instance_buffer +=
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
                instance_buffer +=
                    &format!("{}{}{}\"{}\"\n", pad_date, pad_size, fixed_padding, d_path);
            }
        }
        if !config.opt_no_pretty {
            instance_buffer += &format!("{}\n", fancy_string);
        }
    }

    if config.opt_no_pretty {
        for line in instance_buffer.lines().rev() {
            buffer += &format!("{}\n", line);
        }
    } else {
        buffer += &instance_buffer.to_string();
    };

    Ok(buffer)
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
