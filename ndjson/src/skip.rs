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

//! `skip`: every record after the first *n*.

use crate::{
    head::flush,
    records::{for_each_record, write_record, Flow},
};
use ndjson_common::error::NdJsonSpatialError;
use std::io::{BufRead, Write};

/// Drops the first `n` records of `input` and copies the rest to `output`.
pub fn skip<I: BufRead, O: Write>(
    n: u64,
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    let mut seen = 0;
    for_each_record(input, |record| {
        if seen < n {
            seen += 1;
            return Ok(Flow::Continue);
        }
        Ok(if write_record(output, record)? {
            Flow::Continue
        } else {
            Flow::Stop
        })
    })?;
    flush(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(n: u64, input: &str) -> String {
        let mut output = vec![];
        skip(n, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_skip_drops_first_n() {
        assert_eq!(
            run(2, "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n{\"a\":4}"),
            "{\"a\":3}\n{\"a\":4}\n"
        );
    }

    #[test]
    fn test_skip_zero_and_past_end() {
        assert_eq!(run(0, "{\"a\":1}\n"), "{\"a\":1}\n");
        assert_eq!(run(5, "{\"a\":1}\n{\"a\":2}\n"), "");
    }

    #[test]
    fn test_skip_does_not_count_blank_lines() {
        assert_eq!(run(1, "\n{\"a\":1}\n\n{\"a\":2}\n"), "{\"a\":2}\n");
    }
}
