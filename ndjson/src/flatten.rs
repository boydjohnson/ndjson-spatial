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

//! `flatten` and `unflatten`: nested objects to and from dotted keys,
//! e.g. `{"a":{"b":1}}` and `{"a.b":1}`.

use crate::{
    head::flush,
    records::{flow, for_each_value, write_value, Skipped},
};
use ndjson_common::error::NdJsonSpatialError;
use serde_json::{map::Entry, Map, Value};
use std::io::{BufRead, Write};

/// Rewrites each object record with nested objects (and with `arrays`,
/// arrays, keyed by index) as `sep`-joined keys. Empty objects and arrays
/// stay as values; records that aren't objects are written as they are.
pub fn flatten<I: BufRead, O: Write>(
    sep: &str,
    arrays: bool,
    input: &mut I,
    output: &mut O,
) -> Result<Skipped, NdJsonSpatialError> {
    let skipped = for_each_value(input, |_, record| {
        Ok(Ok(flow(write_value(
            output,
            &flatten_value(record, sep, arrays),
        )?)))
    })?;
    flush(output)?;
    Ok(skipped)
}

/// The inverse of [`flatten`] for objects: splits each key on `sep` and
/// nests the value. A record whose keys collide (`a` and `a.b`) is skipped.
pub fn unflatten<I: BufRead, O: Write>(
    sep: &str,
    input: &mut I,
    output: &mut O,
) -> Result<Skipped, NdJsonSpatialError> {
    let skipped = for_each_value(input, |_, record| match unflatten_value(record, sep) {
        Ok(record) => Ok(Ok(flow(write_value(output, &record)?))),
        Err(reason) => Ok(Err(reason)),
    })?;
    flush(output)?;
    Ok(skipped)
}

pub fn flatten_value(record: Value, sep: &str, arrays: bool) -> Value {
    match record {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                flatten_into(key, value, sep, arrays, &mut out);
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn flatten_into(
    prefix: String,
    value: Value,
    sep: &str,
    arrays: bool,
    out: &mut Map<String, Value>,
) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                flatten_into(format!("{}{}{}", prefix, sep, key), value, sep, arrays, out);
            }
        }
        Value::Array(items) if arrays && !items.is_empty() => {
            for (i, value) in items.into_iter().enumerate() {
                flatten_into(format!("{}{}{}", prefix, sep, i), value, sep, arrays, out);
            }
        }
        value => {
            out.insert(prefix, value);
        }
    }
}

pub fn unflatten_value(record: Value, sep: &str) -> Result<Value, String> {
    let map = match record {
        Value::Object(map) => map,
        other => return Ok(other),
    };
    let mut out = Map::new();
    for (key, value) in map {
        let mut parts = key.split(sep).peekable();
        let mut target = &mut out;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                match target.entry(part) {
                    Entry::Vacant(slot) => {
                        slot.insert(value);
                    }
                    Entry::Occupied(_) => return Err(collision(&key)),
                }
                break;
            }
            target = match target
                .entry(part)
                .or_insert_with(|| Value::Object(Map::new()))
            {
                Value::Object(inner) => inner,
                _ => return Err(collision(&key)),
            };
        }
    }
    Ok(Value::Object(out))
}

fn collision(key: &str) -> String {
    format!("key {:?} collides with another key", key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_flatten_value() {
        let record = json!({"a": {"b": 1, "c": {"d": [1, 2]}}, "e": {}, "f": "x"});
        assert_eq!(
            flatten_value(record.clone(), ".", false),
            json!({"a.b": 1, "a.c.d": [1, 2], "e": {}, "f": "x"})
        );
        assert_eq!(
            flatten_value(record, "_", true),
            json!({"a_b": 1, "a_c_d_0": 1, "a_c_d_1": 2, "e": {}, "f": "x"})
        );
        assert_eq!(
            flatten_value(json!([{"a": {"b": 1}}]), ".", true),
            json!([{"a": {"b": 1}}])
        );
    }

    #[test]
    fn test_flatten_keeps_key_order() {
        let mut out = vec![];
        flatten(
            ".",
            false,
            &mut "{\"z\":{\"y\":1,\"x\":2},\"a\":3}\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"z.y\":1,\"z.x\":2,\"a\":3}\n"
        );
    }

    #[test]
    fn test_unflatten_round_trips() {
        let record = json!({"a": {"b": 1, "c": {"d": [1, 2]}}, "e": {}, "f": "x"});
        let flat = flatten_value(record.clone(), ".", false);
        assert_eq!(unflatten_value(flat, "."), Ok(record));
    }

    #[test]
    fn test_unflatten_collisions_are_skipped() {
        assert!(unflatten_value(json!({"a": 1, "a.b": 2}), ".").is_err());
        assert!(unflatten_value(json!({"a.b": 2, "a": 1}), ".").is_err());
        let mut out = vec![];
        let skipped = unflatten(
            ".",
            &mut "{\"a\":1,\"a.b\":2}\n{\"x.y\":1}\n".as_bytes(),
            &mut out,
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "{\"x\":{\"y\":1}}\n");
        assert_eq!(skipped.count, 1);
        assert_eq!(skipped.first.unwrap().0, 1);
    }
}
