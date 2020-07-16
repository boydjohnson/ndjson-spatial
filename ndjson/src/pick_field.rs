/*
 * Copyright 2019 Boyd Johnson
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

use crate::filter::select_from_json_object;
use ndjson_common::{
    error::NdJsonSpatialError, json_selector_parser::parse_json_selector, ndjson::NdjsonReader,
};
use serde_json::{ser::to_string, Value};
use std::io::Write;

pub fn pick_field(expression: &str) -> Result<(), NdJsonSpatialError> {
    let (_, identifiers) = parse_json_selector(expression.into())
        .map_err(|e| NdJsonSpatialError::Error(format!("Unable to parse expression: {}", e)))?;
    for value in NdjsonReader::default() {
        let v = value?;
        if let Ok(value) = select_from_json_object(v.clone(), &identifiers) {
            match value {
                Value::String(s) => {
                    writeln!(::std::io::stdout(), "{}", s).expect("Unable to write to stdout");
                }
                Value::Number(n) => {
                    writeln!(::std::io::stdout(), "{}", n).expect("Unable to write to stdout");
                }
                Value::Object(o) => {
                    writeln!(
                        ::std::io::stdout(),
                        "{}",
                        to_string(&o).expect("ndjson object failed serialazation")
                    )
                    .expect("Unable to write to stdout");
                }
                Value::Bool(b) => {
                    writeln!(::std::io::stdout(), "{}", b).expect("Unable to write to stdout");
                }
                Value::Array(a) => {
                    for item in &a {
                        writeln!(::std::io::stdout(), "{}", item)
                            .expect("Unable to write to stdout");
                    }
                }
                _ => (),
            }
        }
    }
    Ok(())
}
