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

use crate::error::NdJsonSpatialError;
use geojson::GeoJson;
use serde_json::Value;
use std::io::{stderr, stdin, BufRead, BufReader, Stdin, Write};

/// A struct for reading ndjson geojson from a `BufRead` source,
/// most usually `BufReader<Stdin>` which is provided by `NdjsonGeojsonReader::default()`.
/// Often used as the `Iterator` impl for which `Self::Item = geojson::GeoJson`.
pub struct NdJsonGeojsonReader<IN> {
    std_in: IN,
}

impl<IN> NdJsonGeojsonReader<IN>
where
    IN: BufRead,
{
    pub fn new(std_in: IN) -> Self {
        NdJsonGeojsonReader { std_in }
    }
}

impl<'a> Default for NdJsonGeojsonReader<BufReader<Stdin>> {
    fn default() -> Self {
        let s = stdin();
        Self::new(BufReader::new(s))
    }
}

impl<IN> Iterator for NdJsonGeojsonReader<IN>
where
    IN: BufRead,
{
    type Item = Result<GeoJson, NdJsonSpatialError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match self.std_in.read_line(&mut line) {
            Ok(_) => {
                if line.is_empty() {
                    return None;
                }
                let line = if line.ends_with(',') {
                    let comma_index = line.len() - 1;
                    line[..comma_index].to_string()
                } else {
                    line
                };
                Some(
                    line.parse::<GeoJson>()
                        .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e))),
                )
            }
            Err(e) => {
                writeln!(stderr(), "Error reading ndjson: {}", e)
                    .expect("Unable to write to stderr");

                None
            }
        }
    }
}

/// Reads json items from a ndjson `BufRead`.
/// The `Default` impl gives you the source of
/// `BufReader<Stdin>`. Usually used as the `Iterator` impl,
/// for which `Self::Item = serde_json::Value`.
pub struct NdjsonReader<IN> {
    std_in: IN,
}

impl<IN> NdjsonReader<IN> {
    pub fn new(std_in: IN) -> Self {
        NdjsonReader { std_in }
    }
}

impl<'a> Default for NdjsonReader<BufReader<Stdin>> {
    fn default() -> Self {
        Self::new(BufReader::new(stdin()))
    }
}

impl<IN> Iterator for NdjsonReader<IN>
where
    IN: BufRead,
{
    type Item = Result<Value, NdJsonSpatialError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match self.std_in.read_line(&mut line) {
            Ok(_) => {
                if line.is_empty() {
                    return None;
                }
                let line = if line.ends_with(',') {
                    let comma_index = line.len() - 1;
                    line[..comma_index].to_string()
                } else {
                    line
                };
                Some(
                    serde_json::from_str(&line)
                        .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e))),
                )
            }
            Err(e) => {
                writeln!(stderr(), "Error reading ndjson: {}", e)
                    .expect("Unable to write to stderr");
                None
            }
        }
    }
}
