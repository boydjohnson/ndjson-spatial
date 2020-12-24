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
        parse_selector_string, Compare, ParseValue, Selector,
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

    if let Ok((_, (compare, identifiers))) = parse_selector_i64(expression.as_str().into()) {
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
                let ident = ident
                    .strip_prefix('"')
                    .map(|s| s.strip_suffix('"'))
                    .flatten()
                    .unwrap_or_else(|| ident.as_str());

                if let Value::Array(_) = last_value {
                    return Err(NdJsonSpatialError::Error(format!(
                        "Unable to get attribute {} on array",
                        ident
                    )));
                } else if let Value::Object(value_map) = last_value {
                    last_value = value_map.get(ident).cloned().ok_or_else(|| {
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
    Ok(last_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_i64() {
        let mut input = "{ \"a\": 1 }\n{ \"a\": -45 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a < -40".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":-45}\n".as_bytes(), output.as_slice());

        let mut input = "{ \"a\": 1 }\n{ \"a\": -45 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a > -40".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":1}\n".as_bytes(), output.as_slice());

        let mut input = "{ \"a\": 40250 }\n{ \"a\": -45 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a > 10000".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":40250}\n".as_bytes(), output.as_slice());

        let mut input = "{ \"a\": 40250 }\n{ \"a\": -45 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a < 10000".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":-45}\n".as_bytes(), output.as_slice());
    }

    #[test]
    fn test_filter_bool() {
        let mut input =
            "{ \"a\": true, \"b\": \"foo\" }\n{ \"a\": false, \"b\": \"bar\" }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a == true".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":true,\"b\":\"foo\"}\n".as_bytes(), output.as_slice());

        let mut input =
            "{ \"a\": true, \"b\": \"foo\" }\n{ \"a\": false, \"b\": \"bar\" }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a == false".to_string(), &mut input, &mut output).unwrap();

        assert_eq!(
            "{\"a\":false,\"b\":\"bar\"}\n".as_bytes(),
            output.as_slice()
        );
    }

    #[test]
    fn test_filter_float() {
        let mut input = "{ \"a\": 10.4 }\n{ \"a\": -34.58 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a < 10.4".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":-34.58}\n".as_bytes(), output.as_slice());

        let mut input = "{\"a\": 24 }\n{ \"a\": 54 }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a > 30.0".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":54}\n".as_bytes(), output.as_slice());
    }

    #[test]
    fn test_filter_null() {
        let mut input = "{ \"a\": null }\n{ \"a\": false }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a == null".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":null}\n".as_bytes(), output.as_slice());

        let mut input = "{ \"a\": null }\n{ \"a\": false }\n".as_bytes();

        let mut output = vec![];

        ndjson_filter("d.a != null".to_string(), &mut input, &mut output).unwrap();

        assert_eq!("{\"a\":false}\n".as_bytes(), output.as_slice());
    }
}
