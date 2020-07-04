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
use encode_unicode::IterExt;
use std::io::{BufReader, Read, Write};

const LEFT_BRACE: char = '{';
const RIGHT_BRACE: char = '}';
const NEW_LINE_DELIMITER: char = '\n';

pub fn generic_split<R: Read, W: Write>(
    read: R,
    mut write: W,
    name: &str,
) -> Result<(), NdJsonSpatialError> {
    let mut string_until_feature = String::new();

    let r = BufReader::new(read);
    let mut bytes_iter = r.bytes().scan(0, |_, result| result.ok()).to_utf8chars();

    for byte in &mut bytes_iter {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();

        string_until_feature.push(c);
        if string_until_feature.contains(name) {
            break;
        }
    }
    let mut json_string = String::new();
    let mut start = false;
    let mut count = 0;
    for byte in &mut bytes_iter {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();
        if c == LEFT_BRACE {
            count += 1;
            start = true;
        }
        if start && c != NEW_LINE_DELIMITER {
            json_string.push(c);
        }

        if c == RIGHT_BRACE {
            count -= 1;
        }
        if count == 0 && start {
            if let Err(err) = writeln!(write, "{}", json_string) {
                writeln!(std::io::stderr(), "{}", err).expect("Unable to write to stderr");
            }
            json_string = String::new();
            start = false;
        }
    }

    Ok(())
}
