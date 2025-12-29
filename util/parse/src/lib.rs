// Copyright (c) 2018-2022 The Botho Foundation
// Copyright (c) 2024 Botho Foundation
#![deny(missing_docs)]

//! Miscellaneous parsing and formatting utilities

use core::fmt::Display;
use itertools::Itertools;
use std::{str::FromStr, time::Duration};

mod hex;
pub use crate::hex::parse_hex;

/// Parse a number of seconds into a duration
///
/// This can be used with Clap
pub fn parse_duration_in_seconds(src: &str) -> Result<Duration, std::num::ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(src)?))
}

/// Parse a number of milliseconds into a duration
///
/// This can be used with Clap
pub fn parse_duration_in_millis(src: &str) -> Result<Duration, std::num::ParseIntError> {
    Ok(Duration::from_millis(u64::from_str(src)?))
}

/// Helper to format a sequence as a comma-separated list
/// (This is used with lists of Ingest peer uris in logs,
/// because the debug logging of that object is harder to read)
///
/// To use this, wrap the value in SeqDisplay( ) then format it
pub struct SeqDisplay<T: Display, I: Iterator<Item = T> + Clone>(pub I);

impl<T: Display, I: Iterator<Item = T> + Clone> Display for SeqDisplay<T, I> {
    fn fmt(&self, fmt: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(fmt, "[{}]", self.0.clone().format(", "))
    }
}
