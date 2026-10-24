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

//! Raw record I/O for commands that pass records through untouched.
//! Records are lines; blank lines are not records. Nothing is parsed.

use ndjson_common::error::NdJsonSpatialError;
use std::io::{BufRead, ErrorKind, Write};

pub(crate) enum Flow {
    Continue,
    Stop,
}

/// Calls `f` with each record of `input`, without its line ending, until
/// the input ends or `f` returns `Flow::Stop`.
pub(crate) fn for_each_record<I, F>(input: &mut I, mut f: F) -> Result<(), NdJsonSpatialError>
where
    I: BufRead,
    F: FnMut(&[u8]) -> Result<Flow, NdJsonSpatialError>,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line).map_err(io_error)? == 0 {
            return Ok(());
        }
        let record = line.trim_ascii_end();
        if record.trim_ascii_start().is_empty() {
            continue;
        }
        if let Flow::Stop = f(record)? {
            return Ok(());
        }
    }
}

/// Writes `record` and a newline. Returns `Ok(false)` once the reader of
/// `output` has gone away, so callers can stop quietly like `head(1)`.
pub(crate) fn write_record<O: Write>(
    output: &mut O,
    record: &[u8],
) -> Result<bool, NdJsonSpatialError> {
    match output
        .write_all(record)
        .and_then(|_| output.write_all(b"\n"))
    {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(io_error(e)),
    }
}

pub(crate) fn io_error(e: std::io::Error) -> NdJsonSpatialError {
    NdJsonSpatialError::Error(format!("IO error: {}", e))
}

/// Records a command skipped because it couldn't handle them.
#[derive(Debug, Default, PartialEq)]
pub struct Skipped {
    pub count: u64,
    /// The first one: 1-based record number and why.
    pub first: Option<(u64, String)>,
}

impl Skipped {
    pub(crate) fn add(&mut self, n: u64, reason: String) {
        self.count += 1;
        self.first.get_or_insert((n, reason));
    }
}

/// Calls `f` with the 1-based number and parsed value of each record. A
/// record that isn't JSON is skipped and noted in the returned `Skipped`,
/// as is one for which `f` returns `Err(reason)`.
pub(crate) fn for_each_value<I, F>(input: &mut I, mut f: F) -> Result<Skipped, NdJsonSpatialError>
where
    I: BufRead,
    F: FnMut(u64, serde_json::Value) -> Result<Result<Flow, String>, NdJsonSpatialError>,
{
    let mut skipped = Skipped::default();
    let mut n = 0;
    for_each_record(input, |line| {
        n += 1;
        let value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(e) => {
                skipped.add(n, format!("invalid JSON: {}", e));
                return Ok(Flow::Continue);
            }
        };
        match f(n, value)? {
            Ok(flow) => Ok(flow),
            Err(reason) => {
                skipped.add(n, reason);
                Ok(Flow::Continue)
            }
        }
    })?;
    Ok(skipped)
}

/// Writes `value` as one record; `Ok(false)` once the reader has gone away.
pub(crate) fn write_value<O: Write>(
    output: &mut O,
    value: &serde_json::Value,
) -> Result<bool, NdJsonSpatialError> {
    let line = serde_json::to_vec(value)
        .map_err(|e| NdJsonSpatialError::Error(format!("unable to write a record: {}", e)))?;
    write_record(output, &line)
}

pub(crate) fn flow(written: bool) -> Flow {
    if written {
        Flow::Continue
    } else {
        Flow::Stop
    }
}
