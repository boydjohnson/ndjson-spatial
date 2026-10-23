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

//! `uniq`: collapses runs of adjacent records with equal keys, like
//! `uniq(1)`. Records compare as JSON values (`1` equals `1.0`, object key
//! order doesn't matter), the same equality `sort -u` uses.

use crate::{
    head::flush,
    memcomparable,
    records::{for_each_record, write_record, Flow},
    sort::{record_key, SortKey, SortSummary},
};
use ndjson_common::error::NdJsonSpatialError;
use serde_json::{Map, Value};
use std::io::{BufRead, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// One record per run.
    All,
    /// Only runs of two or more (`uniq -d`).
    Repeated,
    /// Only runs of one (`uniq -u`).
    Unique,
}

#[derive(Debug, Clone)]
pub struct UniqOptions {
    /// Compare these keys; the whole record when empty.
    pub keys: Vec<SortKey>,
    pub keep: Keep,
    /// Add the run length under this field name.
    pub count_field: Option<String>,
}

/// Writes the first record of each run of equal records in `input`.
/// Returns key evaluation errors; a failed key compares as `null`.
pub fn uniq<I: BufRead, O: Write>(
    options: &UniqOptions,
    input: &mut I,
    output: &mut O,
) -> Result<SortSummary, NdJsonSpatialError> {
    let mut summary = SortSummary::default();
    let mut run: Option<Run> = None;
    let mut n = 0;
    for_each_record(input, |line| {
        n += 1;
        let key = if options.keys.is_empty() {
            let record: Value = serde_json::from_slice(line).map_err(|e| {
                NdJsonSpatialError::Error(format!("record {}: invalid JSON: {}", n, e))
            })?;
            let mut key = Vec::new();
            memcomparable::encode(&record, false, &mut key);
            key
        } else {
            record_key(&options.keys, line, n, &mut summary)?
        };
        match &mut run {
            Some(run) if run.key == key => {
                run.count += 1;
                Ok(Flow::Continue)
            }
            _ => {
                let previous = run.replace(Run {
                    key,
                    first: line.to_vec(),
                    count: 1,
                });
                emit(options, previous, output)
            }
        }
    })?;
    emit(options, run, output)?;
    flush(output)?;
    Ok(summary)
}

struct Run {
    key: Vec<u8>,
    first: Vec<u8>,
    count: u64,
}

fn emit<O: Write>(
    options: &UniqOptions,
    run: Option<Run>,
    output: &mut O,
) -> Result<Flow, NdJsonSpatialError> {
    let run = match run {
        Some(run) => run,
        None => return Ok(Flow::Continue),
    };
    let wanted = match options.keep {
        Keep::All => true,
        Keep::Repeated => run.count > 1,
        Keep::Unique => run.count == 1,
    };
    if !wanted {
        return Ok(Flow::Continue);
    }
    let written = match &options.count_field {
        None => write_record(output, &run.first)?,
        Some(field) => {
            let record: Value = serde_json::from_slice(&run.first)
                .map_err(|e| NdJsonSpatialError::Error(format!("invalid JSON: {}", e)))?;
            let line = serde_json::to_vec(&with_count(record, field, run.count))
                .map_err(|e| NdJsonSpatialError::Error(e.to_string()))?;
            write_record(output, &line)?
        }
    };
    Ok(if written { Flow::Continue } else { Flow::Stop })
}

/// Puts the count first, as `uniq -c` does. A record that isn't an object
/// becomes `{"<field>": count, "value": record}`.
fn with_count(record: Value, field: &str, count: u64) -> Value {
    let mut counted = Map::new();
    counted.insert(field.to_string(), Value::from(count));
    match record {
        Value::Object(map) => {
            for (k, v) in map {
                if k != field {
                    counted.insert(k, v);
                }
            }
        }
        other => {
            counted.insert("value".to_string(), other);
        }
    }
    Value::Object(counted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmath_ast::FunctionRegistry;

    fn run(keys: &[&str], keep: Keep, count: bool, input: &str) -> String {
        let registry = FunctionRegistry::default();
        let options = UniqOptions {
            keys: keys
                .iter()
                .map(|k| SortKey::parse(&registry, k).unwrap())
                .collect(),
            keep,
            count_field: if count { Some("count".into()) } else { None },
        };
        let mut output = vec![];
        uniq(&options, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_uniq_drops_adjacent_duplicates_only() {
        let input = "{\"a\":1}\n{\"a\":1}\n{\"a\":2}\n{\"a\":1}\n";
        assert_eq!(
            run(&[], Keep::All, false, input),
            "{\"a\":1}\n{\"a\":2}\n{\"a\":1}\n"
        );
    }

    #[test]
    fn test_uniq_compares_values_not_text() {
        let input = "{\"a\":1,\"b\":2}\n{ \"b\": 2, \"a\": 1.0 }\n";
        assert_eq!(run(&[], Keep::All, false, input), "{\"a\":1,\"b\":2}\n");
    }

    #[test]
    fn test_uniq_by_key_keeps_first_of_run() {
        let input = "{\"k\":\"x\",\"i\":0}\n{\"k\":\"x\",\"i\":1}\n{\"k\":\"y\",\"i\":2}\n";
        assert_eq!(
            run(&["d.k"], Keep::All, false, input),
            "{\"k\":\"x\",\"i\":0}\n{\"k\":\"y\",\"i\":2}\n"
        );
    }

    #[test]
    fn test_uniq_count() {
        let input = "\"a\"\n\"a\"\n\"b\"\n{\"k\":1,\"count\":9}\n{\"k\":1}\n";
        assert_eq!(
            run(&["d"], Keep::All, true, input),
            "{\"count\":2,\"value\":\"a\"}\n{\"count\":1,\"value\":\"b\"}\n{\"count\":1,\"k\":1}\n{\"count\":1,\"k\":1}\n"
        );
        assert_eq!(
            run(
                &["d.k"],
                Keep::All,
                true,
                "{\"k\":1,\"i\":0}\n{\"k\":1,\"i\":1}\n"
            ),
            "{\"count\":2,\"k\":1,\"i\":0}\n"
        );
    }

    #[test]
    fn test_uniq_repeated_and_unique() {
        let input = "1\n1\n2\n3\n3\n3\n4\n";
        assert_eq!(run(&[], Keep::Repeated, false, input), "1\n3\n");
        assert_eq!(run(&[], Keep::Unique, false, input), "2\n4\n");
    }

    #[test]
    fn test_uniq_empty_input() {
        assert_eq!(run(&[], Keep::All, true, ""), "");
    }
}
