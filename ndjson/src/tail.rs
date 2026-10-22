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

//! `tail`: the last *n* records of the stream, held in a ring buffer.

use crate::{
    head::flush,
    records::{for_each_record, write_record, Flow},
};
use ndjson_common::error::NdJsonSpatialError;
use std::{
    collections::VecDeque,
    io::{BufRead, Write},
};

/// Copies the last `n` records of `input` to `output`. Memory is bounded by
/// the `n` records kept.
pub fn tail<I: BufRead, O: Write>(
    n: usize,
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    if n == 0 {
        return Ok(());
    }
    let mut ring: VecDeque<Vec<u8>> = VecDeque::with_capacity(n.min(1024));
    for_each_record(input, |record| {
        let mut buf = if ring.len() == n {
            ring.pop_front().expect("ring is full")
        } else {
            Vec::new()
        };
        buf.clear();
        buf.extend_from_slice(record);
        ring.push_back(buf);
        Ok(Flow::Continue)
    })?;
    for record in &ring {
        if !write_record(output, record)? {
            return Ok(());
        }
    }
    flush(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(n: usize, input: &str) -> String {
        let mut output = vec![];
        tail(n, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_tail_takes_last_n() {
        assert_eq!(
            run(2, "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n{\"a\":4}\n"),
            "{\"a\":3}\n{\"a\":4}\n"
        );
    }

    #[test]
    fn test_tail_shorter_input() {
        assert_eq!(run(10, "{\"a\":1}\n{\"a\":2}\n"), "{\"a\":1}\n{\"a\":2}\n");
    }

    #[test]
    fn test_tail_zero() {
        assert_eq!(run(0, "{\"a\":1}\n"), "");
    }

    #[test]
    fn test_tail_skips_blank_lines_and_adds_final_newline() {
        assert_eq!(
            run(2, "{\"a\":1}\n{\"a\":2}\n\n{\"a\":3}\n\n"),
            "{\"a\":2}\n{\"a\":3}\n"
        );
        assert_eq!(run(1, "{\"a\":1}\n{\"a\":2}"), "{\"a\":2}\n");
    }
}
