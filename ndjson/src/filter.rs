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

use ndjson_common::error::NdJsonSpatialError;
use ndjson_common::json_parser::{
    parse_selector_f64, parse_selector_string, parse_selector_u64, Compare, Identifier,
};
use ndjson_common::ndjson::NdjsonReader;
use serde_json::Value;
use std::io::Write;
use std::str::FromStr;

pub fn ndjson_filter(expression: String) -> Result<(), NdJsonSpatialError> {
    if let Ok((_, (compare, identifiers))) = parse_selector_u64(&expression) {
        write_to_stdout_if_filter_is_true(compare, identifiers)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_f64(&expression) {
        write_to_stdout_if_filter_is_true(compare, identifiers)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_string(&expression) {
        write_to_stdout_if_filter_is_true(compare, identifiers)?;
    }
    Ok(())
}

fn write_to_stdout_if_filter_is_true<T>(
    compare: Compare<T>,
    identifiers: Vec<Identifier>,
) -> Result<(), NdJsonSpatialError>
where
    T: FromStr + PartialOrd,
{
    for value in NdjsonReader::default() {
        let v = value?;
        if let Ok(value) = select_from_json_object(v.clone(), &identifiers) {
            match value {
                Value::String(s) => {
                    if compare.compare(&s) {
                        writeln!(::std::io::stdout(), "{}", v).expect("unable to write to stdout");
                    }
                }
                Value::Number(n) => {
                    if compare.compare(&n.to_string()) {
                        writeln!(::std::io::stdout(), "{}", v).expect("unable to write to stdout");
                    }
                }
                _ => (),
            }
        }
    }
    Ok(())
}

pub fn select_from_json_object(
    value: Value,
    identifiers: &[Identifier],
) -> Result<Value, NdJsonSpatialError> {
    let mut last_value = value;
    for identifier in identifiers {
        match identifier {
            Identifier::Identifier(ident) => {
                if let Value::Array(_) = last_value {
                    return Err(NdJsonSpatialError::Error(format!(
                        "Unable to get attribute {} on array",
                        ident
                    )));
                } else if let Value::Object(value_map) = last_value {
                    last_value = value_map.get(ident.as_str()).cloned().ok_or_else(|| {
                        NdJsonSpatialError::Error(format!("Object has no attribute {}", ident))
                    })?;
                } else {
                    return Err(NdJsonSpatialError::Error(format!(
                        "Unable to get attribute {} on non-object",
                        &ident
                    )));
                }
            }
            Identifier::ArraySelection(selection) => {
                if let Value::Array(array) = last_value {
                    last_value = array.get(selection.index()).cloned().ok_or_else(|| {
                        NdJsonSpatialError::Error("Index out of bounds".to_string())
                    })?;
                } else {
                    return Err(NdJsonSpatialError::Error(
                        "Unable to index non-array".to_string(),
                    ));
                }
            }
        }
    }
    Ok(last_value.to_owned())
}
