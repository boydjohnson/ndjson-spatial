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

//! `head`: the first *n* records of the stream.

use crate::records::{for_each_record, io_error, write_record, Flow};
use ndjson_common::error::NdJsonSpatialError;
use std::io::{BufRead, Write};

/// Copies the first `n` records of `input` to `output` and stops reading,
/// so an upstream producer gets SIGPIPE instead of streaming to the end.
pub fn head<I: BufRead, O: Write>(
    n: usize,
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    if n == 0 {
        return Ok(());
    }
    let mut written = 0;
    for_each_record(input, |record| {
        if !write_record(output, record)? {
            return Ok(Flow::Stop);
        }
        written += 1;
        Ok(if written == n {
            Flow::Stop
        } else {
            Flow::Continue
        })
    })?;
    flush(output)
}

pub(crate) fn flush<O: Write>(output: &mut O) -> Result<(), NdJsonSpatialError> {
    match output.flush() {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(io_error(e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(n: usize, input: &str) -> String {
        let mut output = vec![];
        head(n, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_head_takes_first_n() {
        assert_eq!(
            run(2, "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n"),
            "{\"a\":1}\n{\"a\":2}\n"
        );
    }

    #[test]
    fn test_head_shorter_input() {
        assert_eq!(run(10, "{\"a\":1}\n{\"a\":2}\n"), "{\"a\":1}\n{\"a\":2}\n");
    }

    #[test]
    fn test_head_zero() {
        assert_eq!(run(0, "{\"a\":1}\n"), "");
    }

    #[test]
    fn test_head_skips_blank_lines_and_adds_final_newline() {
        assert_eq!(
            run(2, "\n{\"a\":1}\n  \n{\"a\":2}"),
            "{\"a\":1}\n{\"a\":2}\n"
        );
    }

    #[test]
    fn test_head_stops_reading() {
        let mut input = "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n".as_bytes();
        let mut output = vec![];
        head(1, &mut input, &mut output).unwrap();
        assert_eq!(input, "{\"a\":2}\n{\"a\":3}\n".as_bytes());
    }
}
