/*
 * Copyright 2026 Boyd Johnson
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! `validate`: checks that every non-blank line is one JSON value.

use crate::records::io_error;
use ndjson_common::error::NdJsonSpatialError;
use serde_json::Value;
use std::io::BufRead;

#[derive(Debug, Default, PartialEq)]
pub struct ValidateSummary {
    /// Valid records read.
    pub records: u64,
    /// Lines that weren't JSON.
    pub invalid: u64,
    /// The first of them: 1-based line number (blank lines count) and why.
    pub first_invalid: Option<(u64, String)>,
}

/// Reads `input` until its end, or with `stop_at_first` until the first
/// line that isn't JSON.
pub fn validate<I: BufRead>(
    input: &mut I,
    stop_at_first: bool,
) -> Result<ValidateSummary, NdJsonSpatialError> {
    let mut summary = ValidateSummary::default();
    let mut line = Vec::new();
    let mut n = 0;
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line).map_err(io_error)? == 0 {
            return Ok(summary);
        }
        n += 1;
        if line.trim_ascii().is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(_) => summary.records += 1,
            Err(e) => {
                summary.invalid += 1;
                summary.first_invalid.get_or_insert((n, e.to_string()));
                if stop_at_first {
                    return Ok(summary);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid() {
        let summary = validate(&mut "{\"a\":1}\n\n[1,2]\n\"s\"".as_bytes(), true).unwrap();
        assert_eq!(
            summary,
            ValidateSummary {
                records: 3,
                invalid: 0,
                first_invalid: None
            }
        );
    }

    #[test]
    fn test_validate_stops_at_first() {
        let input = "{\"a\":1}\n\n{\"a\":\n{\"a\":2}\n1 2\n";
        let summary = validate(&mut input.as_bytes(), true).unwrap();
        assert_eq!((summary.records, summary.invalid), (1, 1));
        assert_eq!(summary.first_invalid.unwrap().0, 3);

        let summary = validate(&mut input.as_bytes(), false).unwrap();
        assert_eq!((summary.records, summary.invalid), (2, 2));
        assert_eq!(summary.first_invalid.unwrap().0, 3);
    }
}
