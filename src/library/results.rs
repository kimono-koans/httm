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

// wrap this complex looking error type, which is used everywhere,
// into something more simple looking. This error, FYI, is really easy to use with rayon.
pub type HttmResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct HttmError {
    description: String,
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
            description: value.to_string(),
            source: None,
        }
    }
}

impl From<String> for HttmError {
    fn from(value: String) -> Self {
        HttmError {
            description: value,
            source: None,
        }
    }
}

impl Error for HttmError {}

impl HttmError {
    pub fn new<T: AsRef<str>>(description: T) -> Self {
        HttmError {
            description: description.as_ref().to_string(),
            source: None,
        }
    }
    pub fn with_source<T: AsRef<str>, E: Error>(description: T, err: E) -> Self {
        HttmError {
            description: description.as_ref().to_string(),
            source: Some(err.to_string()),
        }
    }
}

impl fmt::Display for HttmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.source {
            Some(source) => {
                let description = format!("{}\nSOURCE: {}", self.description, source);
                write!(f, "{}", description)
            }
            None => {
                write!(f, "{}", self.description)
            }
        }
    }
}
