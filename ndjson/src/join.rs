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

//! `ndjson join`: a merge join over two inputs sorted by the join key, or,
//! with `--ref`, a hash join that loads the reference file into memory.
//!
//! Keys are tuples of expressions, compared in the order `ndjson sort` uses
//! (see [`OrderedValue`]) through their
//! [`memcomparable`](crate::memcomparable) encoding, so `1` and `1.0` are the
//! same key. A key with a `null` (including a missing field), array or
//! object component never matches, and neither does one that fails to
//! evaluate; those records still count as unmatched for `--left`,
//! `--right`, `--outer` and `--anti`.

use crate::{
    filter::legacy_selector,
    head::flush,
    memcomparable,
    records::{io_error, write_record},
    select::parse_key_list,
};
use ndjson_common::{error::NdJsonSpatialError, json_selector_parser::Selector};
use ordered_float::OrderedFloat;
use rmath_ast::{CompiledExpr, EvalError, FunctionRegistry};
use serde_json::{Map, Number, Value};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    io::{BufRead, Write},
};

/// Which records a join writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    /// Every matching pair.
    Inner,
    /// Every matching pair, and left records without a match as they are.
    Left,
    /// Every matching pair, and right records without a match.
    Right,
    /// Every matching pair, and unmatched records from both sides.
    Outer,
    /// Left records with at least one match, once each, unchanged.
    Semi,
    /// Left records without a match, unchanged.
    Anti,
}

impl JoinType {
    fn keeps_unmatched_left(self) -> bool {
        matches!(self, JoinType::Left | JoinType::Outer | JoinType::Anti)
    }

    fn keeps_unmatched_right(self) -> bool {
        matches!(self, JoinType::Right | JoinType::Outer)
    }

    /// Whether matches are written as left and right merged together.
    fn pairs(self) -> bool {
        !matches!(self, JoinType::Semi | JoinType::Anti)
    }
}

/// Where a right record's fields go in the joined record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RightFields {
    /// Into the left record; on collisions the right value wins.
    Merge,
    /// Into the left record, each key prefixed (`--prefix-right r_`).
    Prefix(String),
    /// The whole right record nested at this path (`--into d.right`).
    Into(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct JoinOptions {
    pub left_on: Vec<CompiledExpr>,
    pub right_on: Vec<CompiledExpr>,
    pub kind: JoinType,
    pub right_fields: RightFields,
    /// The most right records with one key a merge join holds in memory.
    pub max_run: usize,
}

pub const DEFAULT_MAX_RUN: usize = 1_000_000;

impl JoinOptions {
    /// An inner join on `left_on`/`right_on`, which must be the same,
    /// non-zero, length.
    pub fn new(
        left_on: Vec<CompiledExpr>,
        right_on: Vec<CompiledExpr>,
    ) -> Result<Self, NdJsonSpatialError> {
        if left_on.is_empty() {
            return Err(NdJsonSpatialError::Error(
                "join needs at least one key".to_owned(),
            ));
        }
        if left_on.len() != right_on.len() {
            return Err(NdJsonSpatialError::Error(format!(
                "join needs the same number of left keys ({}) and right keys ({})",
                left_on.len(),
                right_on.len()
            )));
        }
        Ok(JoinOptions {
            left_on,
            right_on,
            kind: JoinType::Inner,
            right_fields: RightFields::Merge,
            max_run: DEFAULT_MAX_RUN,
        })
    }
}

/// Parses a comma-separated key list (`--on 'd.state, d.county'`) given
/// to `flag`.
pub fn parse_keys(flag: &str, src: &str) -> Result<Vec<CompiledExpr>, NdJsonSpatialError> {
    let registry = FunctionRegistry::default();
    parse_key_list(flag, src)
        .map_err(NdJsonSpatialError::Error)?
        .iter()
        .map(|expr| {
            registry
                .resolve(expr)
                .map_err(|e| NdJsonSpatialError::Error(format!("{}: {}", flag, e)))
        })
        .collect()
}

/// Parses one key in the old selector syntax of `--ref-fields` and
/// `--stream-fields`.
pub fn parse_legacy_key(flag: &str, src: &str) -> Result<CompiledExpr, NdJsonSpatialError> {
    legacy_selector(src)
        .map(CompiledExpr::Select)
        .ok_or_else(|| NdJsonSpatialError::Error(format!("{}: invalid selector {:?}", flag, src)))
}

/// Parses the `--into` path: a selector made only of field names.
pub fn parse_into(src: &str) -> Result<Vec<String>, NdJsonSpatialError> {
    let invalid = || {
        NdJsonSpatialError::Error(format!(
            "--into: expected a path of field names like d.right, got {:?}",
            src
        ))
    };
    let path = legacy_selector(src).ok_or_else(invalid)?;
    if path.is_empty() {
        return Err(invalid());
    }
    path.into_iter()
        .map(|step| match step {
            Selector::Identifier(name) => Ok(name),
            Selector::Index(_) => Err(invalid()),
        })
        .collect()
}

/// One join input and the name errors call it by.
pub struct Input<'a, R> {
    pub name: &'a str,
    pub reader: R,
}

impl<'a, R: BufRead> Input<'a, R> {
    pub fn new(name: &'a str, reader: R) -> Self {
        Input { name, reader }
    }
}

/// A join key that failed to evaluate.
#[derive(Debug, PartialEq)]
pub struct KeyError {
    pub input: String,
    /// 1-based line number.
    pub line: u64,
    pub error: EvalError,
}

/// What a join noticed along the way.
#[derive(Debug, Default, PartialEq)]
pub struct JoinSummary {
    /// Records whose key failed to evaluate. They never match.
    pub key_errors: u64,
    pub first_key_error: Option<KeyError>,
}

/// Merge joins `left` and `right`, which must both be sorted ascending by
/// their keys (as `ndjson sort -k` sorts them); a record out of order is an
/// error naming its input and line.
///
/// Output follows key order. For each key, every left record is written
/// with each right record in turn, so memory holds the right records of
/// one key at a time, at most `options.max_run` of them.
pub fn merge_join<L: BufRead, R: BufRead, O: Write>(
    left: Input<L>,
    right: Input<R>,
    output: &mut O,
    options: &JoinOptions,
) -> Result<JoinSummary, NdJsonSpatialError> {
    let mut summary = JoinSummary::default();
    let kind = options.kind;
    let mut left = Side::new(left, &options.left_on, true);
    let mut right = Side::new(right, &options.right_on, true);
    let mut l = left.next(&mut summary)?;
    let mut r = right.next(&mut summary)?;

    loop {
        let order = match (&l, &r) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(a), Some(b)) => match a.key.cmp(&b.key) {
                Ordering::Equal if !a.joinable => Ordering::Less,
                order => order,
            },
        };
        match order {
            Ordering::Less => {
                if r.is_none() && !kind.keeps_unmatched_left() {
                    break;
                }
                let row = l.take().expect("left is ahead");
                if kind.keeps_unmatched_left() && !write_record(output, &row.line)? {
                    return Ok(summary);
                }
                l = left.next(&mut summary)?;
            }
            Ordering::Greater => {
                if l.is_none() && !kind.keeps_unmatched_right() {
                    break;
                }
                let row = r.take().expect("right is ahead");
                if kind.keeps_unmatched_right() && !write_right(output, &row, options)? {
                    return Ok(summary);
                }
                r = right.next(&mut summary)?;
            }
            Ordering::Equal => {
                let key = l.as_ref().expect("keys are equal").key.clone();
                let mut run = Vec::new();
                while let Some(row) = r.take() {
                    if row.key != key {
                        r = Some(row);
                        break;
                    }
                    if kind.pairs() {
                        if run.len() == options.max_run {
                            return Err(NdJsonSpatialError::Error(format!(
                                "{} line {}: more than {} records share one join key; \
                                 raise --max-run",
                                right.name, row.line_no, options.max_run
                            )));
                        }
                        run.push(row.record);
                    }
                    r = right.next(&mut summary)?;
                }
                while let Some(row) = l.take() {
                    if row.key != key {
                        l = Some(row);
                        break;
                    }
                    if !write_matches(output, &row, &run, options)? {
                        return Ok(summary);
                    }
                    l = left.next(&mut summary)?;
                }
            }
        }
    }
    flush(output)?;
    Ok(summary)
}

/// Joins each record of `left`, in input order, to the records of
/// `reference` with the same key. `reference` is loaded into memory; neither
/// input needs to be sorted. Supports inner, `--left`, `--semi` and
/// `--anti` joins.
pub fn hash_join<L: BufRead, R: BufRead, O: Write>(
    left: Input<L>,
    reference: Input<R>,
    output: &mut O,
    options: &JoinOptions,
) -> Result<JoinSummary, NdJsonSpatialError> {
    let kind = options.kind;
    if matches!(kind, JoinType::Right | JoinType::Outer) {
        return Err(NdJsonSpatialError::Error(
            "--ref supports inner, --left, --semi and --anti joins; \
             sort both inputs and merge join them for --right or --outer"
                .to_owned(),
        ));
    }
    let mut summary = JoinSummary::default();

    let mut references: HashMap<Vec<u8>, Vec<Map<String, Value>>> = HashMap::new();
    let mut reference = Side::new(reference, &options.right_on, false);
    while let Some(row) = reference.next(&mut summary)? {
        if row.joinable {
            let matches = references.entry(row.key).or_default();
            if kind.pairs() {
                matches.push(row.record);
            }
        }
    }

    let mut left = Side::new(left, &options.left_on, false);
    while let Some(row) = left.next(&mut summary)? {
        let written = match references.get(&row.key).filter(|_| row.joinable) {
            Some(matches) => write_matches(output, &row, matches, options)?,
            None if kind.keeps_unmatched_left() => write_record(output, &row.line)?,
            None => true,
        };
        if !written {
            return Ok(summary);
        }
    }
    flush(output)?;
    Ok(summary)
}

/// Writes what `left` and its matches `right` produce. Returns `Ok(false)`
/// once the output has gone away.
fn write_matches<O: Write>(
    output: &mut O,
    left: &Row,
    right: &[Map<String, Value>],
    options: &JoinOptions,
) -> Result<bool, NdJsonSpatialError> {
    match options.kind {
        JoinType::Semi => write_record(output, &left.line),
        JoinType::Anti => Ok(true),
        _ => {
            for r in right {
                let mut joined = left.record.clone();
                add_right(&mut joined, r, &options.right_fields);
                if !write_object(output, joined)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

/// Writes a right record that matched nothing, shaped as it would be in a
/// joined record.
fn write_right<O: Write>(
    output: &mut O,
    row: &Row,
    options: &JoinOptions,
) -> Result<bool, NdJsonSpatialError> {
    if options.right_fields == RightFields::Merge {
        return write_record(output, &row.line);
    }
    let mut record = Map::new();
    add_right(&mut record, &row.record, &options.right_fields);
    write_object(output, record)
}

fn add_right(joined: &mut Map<String, Value>, right: &Map<String, Value>, fields: &RightFields) {
    match fields {
        RightFields::Merge => {
            for (k, v) in right {
                joined.insert(k.clone(), v.clone());
            }
        }
        RightFields::Prefix(prefix) => {
            for (k, v) in right {
                joined.insert(format!("{}{}", prefix, k), v.clone());
            }
        }
        RightFields::Into(path) => {
            let (last, parents) = path.split_last().expect("--into path is not empty");
            let mut object = joined;
            for name in parents {
                let entry = object
                    .entry(name.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                if !entry.is_object() {
                    *entry = Value::Object(Map::new());
                }
                object = entry.as_object_mut().expect("entry is an object");
            }
            object.insert(last.clone(), Value::Object(right.clone()));
        }
    }
}

fn write_object<O: Write>(
    output: &mut O,
    record: Map<String, Value>,
) -> Result<bool, NdJsonSpatialError> {
    let line = serde_json::to_vec(&Value::Object(record))
        .map_err(|e| NdJsonSpatialError::Error(format!("unable to write a record: {}", e)))?;
    write_record(output, &line)
}

/// A parsed record and its encoded key.
struct Row {
    line_no: u64,
    line: Vec<u8>,
    record: Map<String, Value>,
    key: Vec<u8>,
    joinable: bool,
}

/// Reads the records of one input and computes their keys, optionally
/// checking that the keys never decrease.
struct Side<'a, R> {
    name: &'a str,
    reader: R,
    keys: &'a [CompiledExpr],
    check_order: bool,
    line_no: u64,
    last_key: Option<Vec<u8>>,
}

impl<'a, R: BufRead> Side<'a, R> {
    fn new(input: Input<'a, R>, keys: &'a [CompiledExpr], check_order: bool) -> Self {
        Side {
            name: input.name,
            reader: input.reader,
            keys,
            check_order,
            line_no: 0,
            last_key: None,
        }
    }

    fn next(&mut self, summary: &mut JoinSummary) -> Result<Option<Row>, NdJsonSpatialError> {
        let mut line = Vec::new();
        loop {
            line.clear();
            if self.reader.read_until(b'\n', &mut line).map_err(io_error)? == 0 {
                return Ok(None);
            }
            self.line_no += 1;
            let trimmed = line.trim_ascii();
            if !trimmed.is_empty() {
                line = trimmed.to_vec();
                break;
            }
        }

        let value: Value = serde_json::from_slice(&line)
            .map_err(|e| self.error(format!("invalid JSON: {}", e)))?;
        let mut key = Vec::new();
        let mut joinable = true;
        for expr in self.keys {
            let component = match expr.eval(&value) {
                Ok(v) => v,
                Err(error) => {
                    summary.key_errors += 1;
                    summary.first_key_error.get_or_insert(KeyError {
                        input: self.name.to_owned(),
                        line: self.line_no,
                        error,
                    });
                    joinable = false;
                    Value::Null
                }
            };
            joinable &= !matches!(component, Value::Null | Value::Array(_) | Value::Object(_));
            memcomparable::encode(&component, false, &mut key);
        }
        let record = match value {
            Value::Object(record) => record,
            _ => return Err(self.error("not a JSON object".to_owned())),
        };

        if self.check_order {
            if self.last_key.as_ref().is_some_and(|last| key < *last) {
                return Err(self.error(
                    "not sorted by the join key; sort the input by the join key \
                     (ndjson sort -k ...) or use --ref for an unsorted lookup file"
                        .to_owned(),
                ));
            }
            self.last_key = Some(key.clone());
        }

        Ok(Some(Row {
            line_no: self.line_no,
            line,
            record,
            key,
            joinable,
        }))
    }

    fn error(&self, message: String) -> NdJsonSpatialError {
        NdJsonSpatialError::Error(format!("{} line {}: {}", self.name, self.line_no, message))
    }
}

/// A JSON value with a total order:
/// `null < false < true < numbers < strings < arrays < objects`.
///
/// The derived `Ord` relies on the variant declaration order, so keep the
/// variants in that order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrderedValue {
    Null,
    Bool(bool),
    Number(OrderedNumber),
    String(String),
    Array(Vec<OrderedValue>),
    Object(BTreeMap<String, OrderedValue>),
}

/// A JSON number that compares numerically across its integer and float
/// representations (`-1 < 2 < 2.5 < 3`). `NaN` sorts after every other
/// number and is equal to itself; `-0.0 == 0.0 == 0`.
#[derive(Debug, Clone, Copy)]
pub enum OrderedNumber {
    PosInt(u64),
    NegInt(i64),
    Float(OrderedFloat<f64>),
}

impl Ord for OrderedNumber {
    fn cmp(&self, other: &Self) -> Ordering {
        use OrderedNumber::{Float, NegInt, PosInt};
        match (*self, *other) {
            (PosInt(a), PosInt(b)) => a.cmp(&b),
            (NegInt(a), NegInt(b)) => a.cmp(&b),
            (PosInt(a), NegInt(b)) => i128::from(a).cmp(&i128::from(b)),
            (NegInt(a), PosInt(b)) => i128::from(a).cmp(&i128::from(b)),
            (Float(a), Float(b)) => a.cmp(&b),
            (PosInt(a), Float(b)) => cmp_int_float(i128::from(a), b.0),
            (NegInt(a), Float(b)) => cmp_int_float(i128::from(a), b.0),
            (Float(a), PosInt(b)) => cmp_int_float(i128::from(b), a.0).reverse(),
            (Float(a), NegInt(b)) => cmp_int_float(i128::from(b), a.0).reverse(),
        }
    }
}

impl PartialOrd for OrderedNumber {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for OrderedNumber {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for OrderedNumber {}

/// Exact comparison of an integer (anything in the i64 ∪ u64 range) with an
/// f64, without the precision loss of casting the integer to f64.
fn cmp_int_float(int: i128, float: f64) -> Ordering {
    if float.is_nan() {
        return Ordering::Less;
    }
    // Every i64/u64 lies strictly inside (-2^64, 2^64), so floats outside
    // that range (including the infinities) are decided without casting.
    const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;
    if float >= TWO_POW_64 {
        return Ordering::Less;
    }
    if float <= -TWO_POW_64 {
        return Ordering::Greater;
    }
    let whole = float.trunc();
    // `whole` is an integer-valued f64 inside (-2^64, 2^64), so the cast is
    // exact.
    match int.cmp(&(whole as i128)) {
        Ordering::Equal => {
            let frac = float - whole;
            if frac > 0.0 {
                Ordering::Less
            } else if frac < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        ord => ord,
    }
}

impl From<Number> for OrderedNumber {
    fn from(other: Number) -> Self {
        if let Some(v) = other.as_u64() {
            OrderedNumber::PosInt(v)
        } else if let Some(v) = other.as_i64() {
            OrderedNumber::NegInt(v)
        } else if let Some(v) = other.as_f64() {
            OrderedNumber::Float(OrderedFloat(v))
        } else {
            panic!("Cannot reach this statement");
        }
    }
}

impl From<Value> for OrderedValue {
    fn from(other: Value) -> Self {
        match other {
            Value::String(s) => OrderedValue::String(s),
            Value::Null => OrderedValue::Null,
            Value::Number(n) => OrderedValue::Number(n.into()),
            Value::Bool(b) => OrderedValue::Bool(b),
            Value::Array(arr) => OrderedValue::Array(arr.into_iter().map(|v| v.into()).collect()),
            Value::Object(obj) => {
                let mut map = BTreeMap::default();
                for (k, v) in obj {
                    map.insert(k, v.into());
                }
                OrderedValue::Object(map)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(on: &str, kind: JoinType) -> JoinOptions {
        let keys = parse_keys("--on", on).unwrap();
        let mut options = JoinOptions::new(keys.clone(), keys).unwrap();
        options.kind = kind;
        options
    }

    fn merge(left: &str, right: &str, options: &JoinOptions) -> Result<String, String> {
        let mut output = vec![];
        merge_join(
            Input::new("left", left.as_bytes()),
            Input::new("right", right.as_bytes()),
            &mut output,
            options,
        )
        .map_err(|NdJsonSpatialError::Error(e)| e)?;
        Ok(String::from_utf8(output).unwrap())
    }

    fn hash(left: &str, reference: &str, options: &JoinOptions) -> Result<String, String> {
        let mut output = vec![];
        hash_join(
            Input::new("left", left.as_bytes()),
            Input::new("reference", reference.as_bytes()),
            &mut output,
            options,
        )
        .map_err(|NdJsonSpatialError::Error(e)| e)?;
        Ok(String::from_utf8(output).unwrap())
    }

    /// Runs both join algorithms and checks they agree.
    fn both(left: &str, right: &str, options: &JoinOptions) -> String {
        let merged = merge(left, right, options).unwrap();
        assert_eq!(hash(left, right, options).unwrap(), merged);
        merged
    }

    const LEFT: &str = concat!(
        "{\"id\": null, \"l\": 0}\n",
        "{\"id\": 1, \"l\": 1}\n",
        "{\"id\": 2, \"l\": 2}\n",
        "{\"id\": 2, \"l\": 3}\n",
        "{\"id\": 4, \"l\": 4}\n",
    );
    const RIGHT: &str = concat!(
        "{\"id\": null, \"r\": 0}\n",
        "{\"id\": 2, \"r\": 1}\n",
        "{\"id\": 2.0, \"r\": 2}\n",
        "{\"id\": 3, \"r\": 3}\n",
        "{\"id\": 4, \"r\": 4}\n",
    );

    #[test]
    fn test_inner() {
        assert_eq!(
            both(LEFT, RIGHT, &options("d.id", JoinType::Inner)),
            concat!(
                "{\"id\":2,\"l\":2,\"r\":1}\n",
                "{\"id\":2.0,\"l\":2,\"r\":2}\n",
                "{\"id\":2,\"l\":3,\"r\":1}\n",
                "{\"id\":2.0,\"l\":3,\"r\":2}\n",
                "{\"id\":4,\"l\":4,\"r\":4}\n",
            )
        );
    }

    #[test]
    fn test_left() {
        assert_eq!(
            both(LEFT, RIGHT, &options("d.id", JoinType::Left)),
            concat!(
                "{\"id\": null, \"l\": 0}\n",
                "{\"id\": 1, \"l\": 1}\n",
                "{\"id\":2,\"l\":2,\"r\":1}\n",
                "{\"id\":2.0,\"l\":2,\"r\":2}\n",
                "{\"id\":2,\"l\":3,\"r\":1}\n",
                "{\"id\":2.0,\"l\":3,\"r\":2}\n",
                "{\"id\":4,\"l\":4,\"r\":4}\n",
            )
        );
    }

    #[test]
    fn test_right_and_outer() {
        assert_eq!(
            merge(LEFT, RIGHT, &options("d.id", JoinType::Right)).unwrap(),
            concat!(
                "{\"id\": null, \"r\": 0}\n",
                "{\"id\":2,\"l\":2,\"r\":1}\n",
                "{\"id\":2.0,\"l\":2,\"r\":2}\n",
                "{\"id\":2,\"l\":3,\"r\":1}\n",
                "{\"id\":2.0,\"l\":3,\"r\":2}\n",
                "{\"id\": 3, \"r\": 3}\n",
                "{\"id\":4,\"l\":4,\"r\":4}\n",
            )
        );
        assert_eq!(
            merge(LEFT, RIGHT, &options("d.id", JoinType::Outer)).unwrap(),
            concat!(
                "{\"id\": null, \"l\": 0}\n",
                "{\"id\": null, \"r\": 0}\n",
                "{\"id\": 1, \"l\": 1}\n",
                "{\"id\":2,\"l\":2,\"r\":1}\n",
                "{\"id\":2.0,\"l\":2,\"r\":2}\n",
                "{\"id\":2,\"l\":3,\"r\":1}\n",
                "{\"id\":2.0,\"l\":3,\"r\":2}\n",
                "{\"id\": 3, \"r\": 3}\n",
                "{\"id\":4,\"l\":4,\"r\":4}\n",
            )
        );
    }

    #[test]
    fn test_semi_and_anti() {
        assert_eq!(
            both(LEFT, RIGHT, &options("d.id", JoinType::Semi)),
            concat!(
                "{\"id\": 2, \"l\": 2}\n",
                "{\"id\": 2, \"l\": 3}\n",
                "{\"id\": 4, \"l\": 4}\n",
            )
        );
        assert_eq!(
            both(LEFT, RIGHT, &options("d.id", JoinType::Anti)),
            concat!("{\"id\": null, \"l\": 0}\n", "{\"id\": 1, \"l\": 1}\n")
        );
    }

    #[test]
    fn test_hash_join_keeps_left_order() {
        let left = "{\"id\": 4}\n{\"id\": 1}\n{\"id\": 2}\n";
        let reference = "{\"id\": 2, \"r\": 2}\n{\"id\": 4, \"r\": 4}\n";
        assert_eq!(
            hash(left, reference, &options("d.id", JoinType::Left)).unwrap(),
            "{\"id\":4,\"r\":4}\n{\"id\": 1}\n{\"id\":2,\"r\":2}\n"
        );
    }

    #[test]
    fn test_hash_join_rejects_right_and_outer() {
        assert!(hash("", "", &options("d.id", JoinType::Right)).is_err());
        assert!(hash("", "", &options("d.id", JoinType::Outer)).is_err());
    }

    #[test]
    fn test_composite_key_requires_every_field_to_match() {
        let left = concat!(
            "{\"state\": \"MN\", \"county\": \"Ramsey\"}\n",
            "{\"state\": \"WI\", \"county\": \"Hennepin\"}\n",
        );
        let right = concat!(
            "{\"state\": \"MN\", \"county\": \"Hennepin\", \"pop\": 1}\n",
            "{\"state\": \"MN\", \"county\": \"Ramsey\", \"pop\": 2}\n",
            "{\"state\": \"WI\", \"county\": \"Ramsey\", \"pop\": 3}\n",
        );
        assert_eq!(
            both(left, right, &options("d.state, d.county", JoinType::Inner)),
            "{\"state\":\"MN\",\"county\":\"Ramsey\",\"pop\":2}\n"
        );
    }

    #[test]
    fn test_different_keys_on_each_side() {
        let left_on = parse_keys("--left-on", "d.st").unwrap();
        let right_on = parse_keys("--right-on", "tolower(d.state)").unwrap();
        let options = JoinOptions::new(left_on, right_on).unwrap();
        assert_eq!(
            both(
                "{\"st\": \"mn\"}\n",
                "{\"state\": \"MN\", \"n\": 1}\n",
                &options
            ),
            "{\"st\":\"mn\",\"state\":\"MN\",\"n\":1}\n"
        );
    }

    #[test]
    fn test_keys_must_pair_up() {
        let a = parse_keys("--on", "d.a, d.b").unwrap();
        let b = parse_keys("--on", "d.a").unwrap();
        assert!(JoinOptions::new(a, b).is_err());
        assert!(JoinOptions::new(vec![], vec![]).is_err());
        assert!(parse_keys("--on", "d.a as b").is_err());
        assert!(parse_keys("--on", "sum(d.a)").is_err());
    }

    #[test]
    fn test_arrays_and_objects_never_match() {
        let left = "{\"k\": [1]}\n{\"k\": {\"a\": 1}}\n";
        assert_eq!(both(left, left, &options("d.k", JoinType::Inner)), "");
    }

    #[test]
    fn test_key_errors_never_match_and_are_counted() {
        let left = "{\"k\": \"a\"}\n{\"k\": 1}\n";
        let right = "{\"k\": \"a\"}\n{\"k\": 1, \"r\": 1}\n";
        let mut output = vec![];
        let summary = merge_join(
            Input::new("left", left.as_bytes()),
            Input::new("right", right.as_bytes()),
            &mut output,
            &options("d.k * 2", JoinType::Inner),
        )
        .unwrap();
        assert_eq!(output, b"{\"k\":1,\"r\":1}\n");
        assert_eq!(summary.key_errors, 2);
        let first = summary.first_key_error.unwrap();
        assert_eq!((first.input.as_str(), first.line), ("left", 1));
    }

    #[test]
    fn test_unsorted_input_names_file_and_line() {
        let unsorted = "{\"id\": 1}\n\n{\"id\": 3}\n{\"id\": 2}\n";
        let sorted = "{\"id\": 1}\n{\"id\": 2}\n{\"id\": 3}\n";
        let err = merge(unsorted, sorted, &options("d.id", JoinType::Inner)).unwrap_err();
        assert!(err.starts_with("left line 4: not sorted"), "{}", err);
        let err = merge(sorted, unsorted, &options("d.id", JoinType::Inner)).unwrap_err();
        assert!(err.starts_with("right line 4: not sorted"), "{}", err);
    }

    #[test]
    fn test_inner_join_stops_when_one_side_ends() {
        // the unsorted tail of right is never read
        let right = "{\"id\": 1}\n{\"id\": 5}\n{\"id\": 0}\n";
        assert_eq!(
            merge("{\"id\": 1}\n", right, &options("d.id", JoinType::Inner)).unwrap(),
            "{\"id\":1}\n"
        );
    }

    #[test]
    fn test_max_run() {
        let right = "{\"id\": 1, \"r\": 1}\n{\"id\": 1, \"r\": 2}\n{\"id\": 1, \"r\": 3}\n";
        let mut opts = options("d.id", JoinType::Inner);
        opts.max_run = 2;
        let err = merge("{\"id\": 1}\n", right, &opts).unwrap_err();
        assert!(
            err.starts_with("right line 3: more than 2 records"),
            "{}",
            err
        );
        // semi and anti joins don't hold the run
        opts.kind = JoinType::Semi;
        assert_eq!(
            merge("{\"id\": 1}\n", right, &opts).unwrap(),
            "{\"id\": 1}\n"
        );
    }

    #[test]
    fn test_prefix_right() {
        let mut opts = options("d.id", JoinType::Outer);
        opts.right_fields = RightFields::Prefix("r_".to_owned());
        assert_eq!(
            merge(
                "{\"id\": 1, \"v\": 1}\n",
                "{\"id\": 1, \"v\": 2}\n{\"id\": 2}\n",
                &opts
            )
            .unwrap(),
            "{\"id\":1,\"v\":1,\"r_id\":1,\"r_v\":2}\n{\"r_id\":2}\n"
        );
    }

    #[test]
    fn test_into() {
        let mut opts = options("d.id", JoinType::Outer);
        opts.right_fields = RightFields::Into(parse_into("d.m.right").unwrap());
        assert_eq!(
            merge(
                "{\"id\": 1, \"m\": 5}\n",
                "{\"id\": 1, \"v\": 2}\n{\"id\": 2}\n",
                &opts
            )
            .unwrap(),
            concat!(
                "{\"id\":1,\"m\":{\"right\":{\"id\":1,\"v\":2}}}\n",
                "{\"m\":{\"right\":{\"id\":2}}}\n"
            )
        );
        assert!(parse_into("d.a[0]").is_err());
        assert!(parse_into("d.a + 1").is_err());
    }

    #[test]
    fn test_legacy_keys() {
        let key = parse_legacy_key("--ref-fields", "d.foo").unwrap();
        let options = JoinOptions::new(vec![key.clone()], vec![key]).unwrap();
        assert_eq!(
            hash(
                "{\"foo\": 1, \"bar\": 2}\n",
                "{\"baz\": 3, \"foo\": 1}\n",
                &options
            )
            .unwrap(),
            "{\"foo\":1,\"bar\":2,\"baz\":3}\n"
        );
    }

    #[test]
    fn test_records_must_be_objects() {
        let err = hash("[1]\n", "", &options("d.id", JoinType::Inner)).unwrap_err();
        assert_eq!(err, "left line 1: not a JSON object");
        let err = merge("{\"id\": 1}\n", "nope\n", &options("d.id", JoinType::Inner)).unwrap_err();
        assert!(err.starts_with("right line 1: invalid JSON"), "{}", err);
    }

    #[test]
    fn test_ordered_number() {
        let val: OrderedNumber = serde_json::Number::from_f64(4.5).unwrap().into();

        assert_eq!(val, OrderedNumber::Float(OrderedFloat(4.5)));

        let val: OrderedNumber = serde_json::Number::from(-56).into();

        assert_eq!(val, OrderedNumber::NegInt(-56));

        let val: OrderedNumber = serde_json::Number::from(56).into();

        assert_eq!(val, OrderedNumber::PosInt(56));
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    use serde_json::json;

    fn num(v: Value) -> OrderedNumber {
        match v {
            Value::Number(n) => n.into(),
            _ => panic!("not a number"),
        }
    }

    #[test]
    fn test_numbers_compare_numerically_across_representations() {
        let mut values = vec![
            num(json!(3)),
            num(json!(2.5)),
            num(json!(-1)),
            num(json!(5)),
            num(json!(-2.5)),
            num(json!(0)),
        ];
        values.sort();
        assert_eq!(
            values,
            vec![
                num(json!(-2.5)),
                num(json!(-1)),
                num(json!(0)),
                num(json!(2.5)),
                num(json!(3)),
                num(json!(5)),
            ]
        );
    }

    #[test]
    fn test_int_and_float_equal_values_are_equal() {
        assert_eq!(num(json!(3)), num(json!(3.0)));
        assert_eq!(num(json!(-7)), num(json!(-7.0)));
        assert_eq!(num(json!(0)), num(json!(-0.0)));
        assert!(num(json!(3)) < num(json!(3.0000001)));
        assert!(num(json!(-3)) > num(json!(-3.0000001)));
    }

    #[test]
    fn test_large_integers_compare_exactly_against_floats() {
        // 2^53 + 1 is not representable as f64; casting would make it equal.
        let big = OrderedNumber::PosInt((1 << 53) + 1);
        let float = OrderedNumber::Float(OrderedFloat((1u64 << 53) as f64));
        assert!(big > float);
        assert!(OrderedNumber::PosInt(u64::MAX) < OrderedNumber::Float(OrderedFloat(1e30)));
        assert!(OrderedNumber::NegInt(i64::MIN) > OrderedNumber::Float(OrderedFloat(-1e30)));
        assert!(OrderedNumber::PosInt(u64::MAX) > OrderedNumber::NegInt(i64::MIN));
    }

    #[test]
    fn test_nan_and_infinity() {
        let nan = OrderedNumber::Float(OrderedFloat(f64::NAN));
        let inf = OrderedNumber::Float(OrderedFloat(f64::INFINITY));
        let neg_inf = OrderedNumber::Float(OrderedFloat(f64::NEG_INFINITY));
        assert!(nan > inf);
        assert!(nan > OrderedNumber::PosInt(u64::MAX));
        assert_eq!(nan, nan);
        assert!(inf > OrderedNumber::PosInt(u64::MAX));
        assert!(neg_inf < OrderedNumber::NegInt(i64::MIN));
    }

    #[test]
    fn test_value_type_order() {
        let mut values: Vec<OrderedValue> = vec![
            json!({"a": 1}),
            json!([1]),
            json!("a"),
            json!(1),
            json!(true),
            json!(false),
            json!(null),
        ]
        .into_iter()
        .map(OrderedValue::from)
        .collect();
        values.sort();
        let expected: Vec<OrderedValue> = vec![
            json!(null),
            json!(false),
            json!(true),
            json!(1),
            json!("a"),
            json!([1]),
            json!({"a": 1}),
        ]
        .into_iter()
        .map(OrderedValue::from)
        .collect();
        assert_eq!(values, expected);
    }
}
