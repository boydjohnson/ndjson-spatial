/*
 * Copyright 2019 Gobsmacked Labs, LLC
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

//! `to-json`: the stream as one JSON array. Records pass through unparsed.

use crate::{
    head::flush,
    records::{for_each_record, io_error, Flow},
};
use ndjson_common::error::NdJsonSpatialError;
use std::io::{BufRead, ErrorKind, Write};

/// Writes the records of `input` as the elements of one array, one per
/// line: `[`, the records separated by commas, then `]`.
pub fn to_json<I: BufRead, O: Write>(
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    let mut first = true;
    let mut gone = false;
    for_each_record(input, |record| {
        let sep: &[u8] = if first { b"[\n" } else { b",\n" };
        first = false;
        match output.write_all(sep).and_then(|_| output.write_all(record)) {
            Ok(()) => Ok(Flow::Continue),
            Err(e) if e.kind() == ErrorKind::BrokenPipe => {
                gone = true;
                Ok(Flow::Stop)
            }
            Err(e) => Err(io_error(e)),
        }
    })?;
    if gone {
        return Ok(());
    }
    let end: &[u8] = if first { b"[]\n" } else { b"\n]\n" };
    match output.write_all(end) {
        Err(e) if e.kind() != ErrorKind::BrokenPipe => return Err(io_error(e)),
        _ => {}
    }
    flush(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let mut output = vec![];
        to_json(&mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_to_json() {
        let out = run("{\"a\":1}\n\n{\"a\":2}");
        assert_eq!(out, "[\n{\"a\":1},\n{\"a\":2}\n]\n");
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value, serde_json::json!([{"a": 1}, {"a": 2}]));
    }

    #[test]
    fn test_to_json_empty() {
        assert_eq!(run(""), "[]\n");
    }
}
