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
    cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl<T> From<HttmError> for HttmResult<T> {
    fn from(value: HttmError) -> Self {
        Err(Box::new(value))
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for HttmError {
    fn from(value: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self {
            details: value.to_string(),
            cause: value.into(),
        }
    }
}

impl From<String> for HttmError {
    fn from(value: String) -> Self {
        HttmError {
            details: value,
            cause: None,
        }
    }
}

impl Error for HttmError {
    fn description(&self) -> &str {
        &self.details
    }
}

impl From<IoError> for HttmError {
    fn from(err: IoError) -> Self {
        HttmError {
            details: err.to_string(),
            cause: Some(err.into()),
        }
    }
}

impl HttmError {
    pub fn new<T: AsRef<str>>(msg: T) -> Self {
        HttmError {
            details: msg.as_ref().to_string(),
            cause: None,
        }
    }
    pub fn with_cause<T: AsRef<str>>(
        msg: T,
        err: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        HttmError {
            details: msg.as_ref().to_string(),
            cause: Some(err),
        }
    }
}

impl fmt::Display for HttmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.cause {
            Some(cause) => {
                let msg = format!("{} : {:?}", self.details, cause);
                write!(f, "{}", msg)
            }
            None => {
                write!(f, "{}", self.details)
            }
        }
    }
}
