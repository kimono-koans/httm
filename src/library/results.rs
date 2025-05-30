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

use std::error::Error;
use std::fmt;
use std::io::Error as IoError;

// wrap this complex looking error type, which is used everywhere,
// into something more simple looking. This error, FYI, is really easy to use with rayon.
pub type HttmResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct HttmError {
    details: String,
    source: Option<String>,
}

impl<T> From<HttmError> for HttmResult<T> {
    fn from(value: HttmError) -> Self {
        Err(Box::new(value))
    }
}

impl From<Box<dyn Error + Send + Sync>> for HttmError {
    fn from(value: Box<dyn Error + Send + Sync>) -> Self {
        Self {
            details: value.to_string(),
            source: None,
        }
    }
}

impl From<String> for HttmError {
    fn from(value: String) -> Self {
        HttmError {
            details: value,
            source: None,
        }
    }
}

impl Error for HttmError {}

impl From<IoError> for HttmError {
    fn from(err: IoError) -> Self {
        HttmError {
            details: err.to_string(),
            source: None,
        }
    }
}

impl HttmError {
    pub fn new<T: AsRef<str>>(msg: T) -> Self {
        HttmError {
            details: msg.as_ref().to_string(),
            source: None,
        }
    }
    pub fn with_source<T: AsRef<str>, E: Error>(msg: T, err: E) -> Self {
        HttmError {
            details: msg.as_ref().to_string(),
            source: Some(err.to_string()),
        }
    }
}

impl fmt::Display for HttmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.source {
            Some(source) => {
                let msg = format!("{} : {:?}", self.details, source);
                write!(f, "{}", msg)
            }
            None => {
                write!(f, "{}", self.details)
            }
        }
    }
}
