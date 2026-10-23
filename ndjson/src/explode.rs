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

//! `explode` (unnest): one output record per element of an array field.

use crate::{
    filter::legacy_selector,
    head::flush,
    records::{flow, for_each_value, write_value, Flow, Skipped},
};
use ndjson_common::{error::NdJsonSpatialError, json_selector_parser::Selector};
use serde_json::Value;
use std::io::{BufRead, Write};

/// Parses the selector of the array field, e.g. `d.tags` or `d.a[0].b`.
pub fn parse_path(selector: &str) -> Result<Vec<Selector>, NdJsonSpatialError> {
    match legacy_selector(selector) {
        Some(path) if !path.is_empty() => Ok(path),
        Some(_) => Err(NdJsonSpatialError::Error(
            "the selector must name a field, not the whole record".to_string(),
        )),
        None => Err(NdJsonSpatialError::Error(format!(
            "invalid selector {:?}",
            selector
        ))),
    }
}

/// For each record of `input`, writes one copy per element of the array at
/// `path`, with the element in place of the array. A record whose field is
/// missing, null or an empty array is dropped, or with `keep_empty` written
/// once with the field null (or as it is, when missing). A field that is
/// some other value is written as it is.
pub fn explode<I: BufRead, O: Write>(
    path: &[Selector],
    keep_empty: bool,
    input: &mut I,
    output: &mut O,
) -> Result<Skipped, NdJsonSpatialError> {
    let skipped = for_each_value(input, |_, mut record| {
        let items = match slot(&mut record, path) {
            None => {
                return Ok(Ok(if keep_empty {
                    flow(write_value(output, &record)?)
                } else {
                    Flow::Continue
                }))
            }
            Some(slot) => match slot.take() {
                Value::Array(items) => items,
                Value::Null => vec![],
                other => vec![other],
            },
        };
        if items.is_empty() {
            return Ok(Ok(if keep_empty {
                flow(write_value(output, &record)?)
            } else {
                Flow::Continue
            }));
        }
        for item in items {
            *slot(&mut record, path).expect("the slot was found above") = item;
            if !write_value(output, &record)? {
                return Ok(Ok(Flow::Stop));
            }
        }
        Ok(Ok(Flow::Continue))
    })?;
    flush(output)?;
    Ok(skipped)
}

fn slot<'a>(mut value: &'a mut Value, path: &[Selector]) -> Option<&'a mut Value> {
    for step in path {
        value = match step {
            Selector::Identifier(key) => value.as_object_mut()?.get_mut(key.as_str())?,
            Selector::Index(i) => value.as_array_mut()?.get_mut(*i)?,
        };
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(selector: &str, keep_empty: bool, input: &str) -> (String, Skipped) {
        let path = parse_path(selector).unwrap();
        let mut output = vec![];
        let skipped = explode(&path, keep_empty, &mut input.as_bytes(), &mut output).unwrap();
        (String::from_utf8(output).unwrap(), skipped)
    }

    #[test]
    fn test_explode_array() {
        let (out, skipped) = run("d.tags", false, "{\"id\":1,\"tags\":[\"a\",\"b\"]}\n");
        assert_eq!(
            out,
            "{\"id\":1,\"tags\":\"a\"}\n{\"id\":1,\"tags\":\"b\"}\n"
        );
        assert_eq!(skipped, Skipped::default());
    }

    #[test]
    fn test_explode_nested_path() {
        let (out, _) = run("d.a.b", false, "{\"a\":{\"b\":[1,2],\"c\":0}}\n");
        assert_eq!(
            out,
            "{\"a\":{\"b\":1,\"c\":0}}\n{\"a\":{\"b\":2,\"c\":0}}\n"
        );
    }

    #[test]
    fn test_explode_empty_missing_and_null() {
        let input = "{\"t\":[]}\n{\"u\":1}\n{\"t\":null}\n{\"t\":5}\n";
        assert_eq!(run("d.t", false, input).0, "{\"t\":5}\n");
        assert_eq!(
            run("d.t", true, input).0,
            "{\"t\":null}\n{\"u\":1}\n{\"t\":null}\n{\"t\":5}\n"
        );
    }

    #[test]
    fn test_explode_skips_invalid_json() {
        let (out, skipped) = run("d.t", false, "nope\n{\"t\":[1]}\n");
        assert_eq!(out, "{\"t\":1}\n");
        assert_eq!(skipped.count, 1);
        assert_eq!(skipped.first.unwrap().0, 1);
    }

    #[test]
    fn test_explode_needs_a_field() {
        assert!(parse_path("d").is_err());
        assert!(parse_path("x y").is_err());
    }
}
