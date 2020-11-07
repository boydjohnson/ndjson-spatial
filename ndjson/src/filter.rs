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

use ndjson_common::{
    error::NdJsonSpatialError,
    json_selector_parser::{
        parse_selector_bool, parse_selector_f64, parse_selector_i64, parse_selector_null,
        parse_selector_string, parse_selector_u64, Compare, ParseValue, Selector,
    },
    ndjson::NdjsonReader,
};
use serde_json::Value;
use std::io::{BufRead, BufReader, BufWriter, Write};

pub fn ndjson_filter<R: BufRead, W: Write>(
    expression: String,
    read: &mut R,
    write: &mut W,
) -> Result<(), NdJsonSpatialError> {
    let mut read = BufReader::with_capacity(1_000_000, read);
    let mut write = BufWriter::with_capacity(1_000_000, write);

    if let Ok((_, (compare, identifiers))) = parse_selector_u64(expression.as_str().into()) {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_i64(expression.as_str().into()) {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_f64(expression.as_str().into()) {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_bool(expression.as_str().into())
    {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    } else if let Ok((_, (compare, identifiers))) = parse_selector_null(expression.as_str().into())
    {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    } else if let Ok((_, (compare, identifiers))) =
        parse_selector_string(expression.as_str().into())
    {
        write_to_stdout_if_filter_is_true(compare, identifiers, &mut read, &mut write)?;
    }
    Ok(())
}

fn write_to_stdout_if_filter_is_true<T, R: BufRead, W: Write>(
    compare: Compare<T>,
    identifiers: Vec<Selector>,
    read: &mut R,
    write: &mut W,
) -> Result<(), NdJsonSpatialError>
where
    T: ParseValue,
{
    for value in NdjsonReader::new(read) {
        let v = value?;
        if let Ok(value) = select_from_json_object(v.clone(), &identifiers) {
            if compare.compare(value) {
                writeln!(write, "{}", v).expect("unable to write to stdout");
            }
        }
    }
    Ok(())
}

pub fn select_from_json_object(
    value: Value,
    identifiers: &[Selector],
) -> Result<Value, NdJsonSpatialError> {
    let mut last_value = value;
    for identifier in identifiers {
        match identifier {
            Selector::Identifier(ident) => {
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
            Selector::Index(selection) => {
                if let Value::Array(array) = last_value {
                    last_value = array.get(*selection).cloned().ok_or_else(|| {
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
