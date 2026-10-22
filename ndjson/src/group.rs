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

//! Run an [`AggregateList`] over a stream, as one group or grouped by key
//! (see `plans/03-grouping-and-aggregation.md`).
//!
//! - [`Grouping::Stream`]: one group, one output record at the end, even for
//!   empty input. Constant memory.
//! - [`Grouping::Sorted`]: groups are runs of equal keys, emitted as each
//!   run ends, so memory is constant however many groups there are. By
//!   default the keys must be non-decreasing, which catches unsorted input.
//! - [`Grouping::Hash`]: any order; one state per distinct key, emitted in
//!   key order at the end. Memory grows with the number of groups, up to a
//!   limit.

use crate::join::OrderedValue;
use ndjson_common::{error::NdJsonSpatialError, ndjson::NdjsonReader};
use rmath_ast::{AggregateList, AggregateState, CompiledExpr, EvalError};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    Stream,
    Sorted {
        /// Fail on a key smaller than the one before it. Without the check,
        /// a key that comes back after its run ended starts a new group.
        check_order: bool,
    },
    Hash {
        max_groups: usize,
    },
}

/// What to compute: output field names, one per item of `list`, and the
/// group key expressions (empty for [`Grouping::Stream`]).
pub struct Aggregation {
    pub names: Vec<String>,
    pub list: AggregateList,
    pub by: Vec<CompiledExpr>,
    pub grouping: Grouping,
}

/// Values that failed to evaluate. Those records are skipped by the
/// aggregate (or, for a key, the whole group-by) they failed in.
#[derive(Debug, Default, PartialEq)]
pub struct AggregateSummary {
    pub errors: u64,
    /// The first failure: 1-based record number, the output field (or
    /// `"group key"`), and the error.
    pub first_error: Option<(u64, String, EvalError)>,
}

impl AggregateSummary {
    fn record(&mut self, record: u64, name: &str, e: EvalError) {
        self.errors += 1;
        self.first_error
            .get_or_insert_with(|| (record, name.to_string(), e));
    }
}

type Key = Vec<OrderedValue>;

pub fn group<R: BufRead, W: Write>(
    aggregation: &Aggregation,
    read: R,
    write: &mut W,
) -> Result<AggregateSummary, NdJsonSpatialError> {
    let Aggregation {
        names,
        list,
        by,
        grouping,
    } = aggregation;
    assert_eq!(names.len(), list.len(), "one name per item");

    let mut summary = AggregateSummary::default();
    let mut emit = |state: &AggregateState, n: u64, summary: &mut AggregateSummary| {
        let mut row = Map::new();
        for (name, value) in names.iter().zip(list.finish(state)) {
            let value = value.unwrap_or_else(|e| {
                summary.record(n, name, e);
                Value::Null
            });
            row.insert(name.clone(), value);
        }
        writeln!(write, "{}", Value::Object(row))
            .map_err(|e| NdJsonSpatialError::Error(format!("IO error: {}", e)))
    };

    // the current run's key, as compared and as read
    let mut current: Option<(Key, Vec<Value>, AggregateState)> = None;
    let mut groups: BTreeMap<Key, AggregateState> = BTreeMap::new();
    let mut whole = list.start();
    let mut n = 0;

    for record in NdjsonReader::new(read) {
        let record = record?;
        n += 1;

        let state = match grouping {
            Grouping::Stream => &mut whole,
            Grouping::Sorted { check_order } => {
                let Some(raw) = group_key(by, &record, n, &mut summary) else {
                    continue;
                };
                let key: Key = raw.iter().cloned().map(OrderedValue::from).collect();
                match &current {
                    Some((k, _, _)) if *k == key => {}
                    previous => {
                        if let Some((k, previous_raw, state)) = previous {
                            if *check_order && key < *k {
                                return Err(NdJsonSpatialError::Error(format!(
                                    "record {}: group key {} sorts before the previous key {}; \
                                     the input must be sorted by the group key",
                                    n,
                                    show(&raw),
                                    show(previous_raw)
                                )));
                            }
                            emit(state, n - 1, &mut summary)?;
                        }
                        current = Some((key, raw, list.start()));
                    }
                }
                &mut current.as_mut().expect("just set").2
            }
            Grouping::Hash { max_groups } => {
                let Some(raw) = group_key(by, &record, n, &mut summary) else {
                    continue;
                };
                let key: Key = raw.into_iter().map(OrderedValue::from).collect();
                if !groups.contains_key(&key) && groups.len() >= *max_groups {
                    return Err(NdJsonSpatialError::Error(format!(
                        "more than {} groups; raise the limit, or sort the input by the \
                         group key and group sorted runs instead",
                        max_groups
                    )));
                }
                groups.entry(key).or_insert_with(|| list.start())
            }
        };
        list.update(state, &record, |item, e| summary.record(n, &names[item], e));
    }

    match grouping {
        Grouping::Stream => emit(&whole, n, &mut summary)?,
        Grouping::Sorted { .. } => {
            if let Some((_, _, state)) = &current {
                emit(state, n, &mut summary)?;
            }
        }
        Grouping::Hash { .. } => {
            for state in groups.values() {
                emit(state, n, &mut summary)?;
            }
        }
    }
    Ok(summary)
}

/// The values of the group key of `record`, or `None` (counted as an
/// error) when one fails to evaluate.
fn group_key(
    by: &[CompiledExpr],
    record: &Value,
    n: u64,
    summary: &mut AggregateSummary,
) -> Option<Vec<Value>> {
    match by.iter().map(|k| k.eval(record)).collect() {
        Ok(key) => Some(key),
        Err(e) => {
            summary.record(n, "group key", e);
            None
        }
    }
}

fn show(key: &[Value]) -> String {
    let values: Vec<_> = key.iter().map(Value::to_string).collect();
    values.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmath_ast::{parser::ExprParser, FunctionRegistry};

    fn aggregation(items: &[&str], by: &[&str], grouping: Grouping) -> Aggregation {
        let registry = FunctionRegistry::default();
        let parse = |s: &&str| *ExprParser::new().parse(s).unwrap();
        let items: Vec<_> = items.iter().map(parse).collect();
        let by: Vec<_> = by.iter().map(parse).collect();
        Aggregation {
            names: (0..items.len()).map(|i| format!("c{}", i)).collect(),
            list: AggregateList::compile(&registry, &items, &by).unwrap(),
            by: by.iter().map(|b| registry.resolve(b).unwrap()).collect(),
            grouping,
        }
    }

    fn run(
        items: &[&str],
        by: &[&str],
        grouping: Grouping,
        input: &str,
    ) -> Result<(Vec<Value>, AggregateSummary), NdJsonSpatialError> {
        let mut out = vec![];
        let summary = group(
            &aggregation(items, by, grouping),
            input.as_bytes(),
            &mut out,
        )?;
        let rows = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        Ok((rows, summary))
    }

    const SORTED: &str = r#"{"s": "IA", "x": 1}
{"s": "IA", "x": 2}
{"s": "MN", "x": 10}
{"s": "MN"}
{"s": "WI", "x": 5}
"#;

    #[test]
    fn test_whole_stream() {
        let (rows, summary) = run(
            &["count()", "sum(d.x)", "max(d.x)", "d.s"],
            &[],
            Grouping::Stream,
            SORTED,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![serde_json::json!({"c0": 5, "c1": 18, "c2": 10, "c3": "MN"})]
        );
        assert_eq!(summary, AggregateSummary::default());
    }

    #[test]
    fn test_empty_stream_emits_one_record() {
        let (rows, _) = run(&["count()", "sum(d.x)"], &[], Grouping::Stream, "").unwrap();
        assert_eq!(rows, vec![serde_json::json!({"c0": 0, "c1": null})]);
        // grouped empty input has no groups
        for grouping in [
            Grouping::Sorted { check_order: true },
            Grouping::Hash { max_groups: 10 },
        ] {
            let (rows, _) = run(&["count()"], &["d.s"], grouping, "").unwrap();
            assert!(rows.is_empty());
        }
    }

    #[test]
    fn test_sorted_runs() {
        let expected = vec![
            serde_json::json!({"c0": "IA", "c1": 2, "c2": 3}),
            serde_json::json!({"c0": "MN", "c1": 2, "c2": 10}),
            serde_json::json!({"c0": "WI", "c1": 1, "c2": 5}),
        ];
        for grouping in [
            Grouping::Sorted { check_order: true },
            Grouping::Sorted { check_order: false },
            Grouping::Hash { max_groups: 3 },
        ] {
            let (rows, _) =
                run(&["d.s", "count()", "sum(d.x)"], &["d.s"], grouping, SORTED).unwrap();
            assert_eq!(rows, expected, "{:?}", grouping);
        }
    }

    #[test]
    fn test_unsorted_input() {
        let input = "{\"s\": \"MN\"}\n{\"s\": \"IA\"}\n{\"s\": \"MN\"}\n";
        let err = run(
            &["d.s", "count()"],
            &["d.s"],
            Grouping::Sorted { check_order: true },
            input,
        )
        .unwrap_err();
        assert_eq!(
            err,
            NdJsonSpatialError::Error(
                "record 2: group key \"IA\" sorts before the previous key \"MN\"; the input must be \
                 sorted by the group key"
                    .into()
            )
        );
        // without the check, every run is its own group
        let (rows, _) = run(
            &["d.s", "count()"],
            &["d.s"],
            Grouping::Sorted { check_order: false },
            input,
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        // hashing groups them, in key order
        let (rows, _) = run(
            &["d.s", "count()"],
            &["d.s"],
            Grouping::Hash { max_groups: 2 },
            input,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                serde_json::json!({"c0": "IA", "c1": 1}),
                serde_json::json!({"c0": "MN", "c1": 2})
            ]
        );
    }

    #[test]
    fn test_composite_keys_and_numeric_equality() {
        let input = r#"{"a": 1, "b": "x"}
{"a": 1.0, "b": "x"}
{"a": 1, "b": "y"}
{"a": 2, "b": "a"}
"#;
        let (rows, _) = run(
            &["d.a", "d.b", "count()"],
            &["d.a", "d.b"],
            Grouping::Sorted { check_order: true },
            input,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                serde_json::json!({"c0": 1, "c1": "x", "c2": 2}),
                serde_json::json!({"c0": 1, "c1": "y", "c2": 1}),
                serde_json::json!({"c0": 2, "c1": "a", "c2": 1}),
            ]
        );
    }

    #[test]
    fn test_max_groups() {
        let err = run(
            &["count()"],
            &["d.s"],
            Grouping::Hash { max_groups: 2 },
            SORTED,
        )
        .unwrap_err();
        assert!(matches!(err, NdJsonSpatialError::Error(e) if e.starts_with("more than 2 groups")));
    }

    #[test]
    fn test_errors_are_counted() {
        let input = "{\"s\": 1, \"x\": \"a\"}\n{\"s\": 1, \"x\": 2}\n{\"s\": \"q\", \"x\": 3}\n";
        let (rows, summary) = run(
            &["sum(d.x)", "count()"],
            &["d.s * 2"],
            Grouping::Hash { max_groups: 10 },
            input,
        )
        .unwrap();
        // record 1 is skipped by sum only; record 3 has no key
        assert_eq!(rows, vec![serde_json::json!({"c0": 2, "c1": 2})]);
        assert_eq!(summary.errors, 2);
        assert_eq!(
            summary.first_error,
            Some((
                1,
                "c0".to_string(),
                EvalError::Type {
                    op: "sum",
                    left: "string",
                    right: None
                }
            ))
        );
    }
}
