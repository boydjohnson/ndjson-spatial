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
use std::io::{BufRead, BufReader, BufWriter, StdoutLock, Write};
use yajlish::{
    ndjson_handler::{NdJsonHandler, Selector},
    Parser,
};

pub fn generic_split<R: BufRead>(
    read: &mut R,
    write: StdoutLock,
    selectors: Vec<Selector>,
) -> Result<(), NdJsonSpatialError> {
    let mut buf_write = BufWriter::with_capacity(5_000_000, write);

    let mut handler = NdJsonHandler::new(&mut buf_write, selectors);

    let mut input = BufReader::with_capacity(1_000_000, read);

    let mut parser = Parser::new(&mut handler);

    let v = parser
        .parse(&mut input)
        .map_err(|e| NdJsonSpatialError::Error(format!("{}", e)));

    buf_write
        .flush()
        .map_err(|e| NdJsonSpatialError::Error(format!("{}", e)))?;

    v
}
