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
use std::io::{Read, Write};

pub fn split() -> Result<(), NdJsonSpatialError> {
    let mut string_until_feature = String::new();

    for byte in ::std::io::stdin().bytes() {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();

        string_until_feature.push(c);
        if string_until_feature.contains("features") {
            break;
        }
    }
    let mut json_string = String::new();
    let mut start = false;
    let mut count = 0;
    for byte in ::std::io::stdin().bytes() {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();
        if c == '{' {
            count += 1;
            start = true;
        }
        if start && c != '\n' {
            json_string.push(c);
        }

        if c == '}' {
            count -= 1;
        }
        if count == 0 && start {
            writeln!(::std::io::stdout(), "{}", json_string).expect("Error writing to stdout");
            json_string = String::new();
            start = false;
        }
    }
    Ok(())
}
