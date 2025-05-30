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
        }
    }
}

impl From<String> for HttmError {
    fn from(value: String) -> Self {
        HttmError { details: value }
    }
}

impl HttmError {
    pub fn new<T: AsRef<str>>(msg: T) -> Self {
        HttmError {
            details: msg.as_ref().to_string(),
        }
    }
    pub fn with_context<T: AsRef<str>>(msg: T, err: &dyn Error) -> Self {
        let msg_plus_context = format!("{} : {err:?}", msg.as_ref());

        HttmError {
            details: msg_plus_context,
        }
    }
}

impl fmt::Display for HttmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.details)
    }
}

impl Error for HttmError {
    fn description(&self) -> &str {
        &self.details
    }
}

impl From<&dyn Error> for HttmError {
    fn from(err: &dyn Error) -> Self {
        let context = format!("{err:?}");
        HttmError { details: context }
    }
}

impl From<IoError> for HttmError {
    fn from(err: IoError) -> Self {
        let context = format!("{err:?}");
        HttmError { details: context }
    }
}
