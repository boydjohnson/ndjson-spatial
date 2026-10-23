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

//! `ndjson select <list>`: evaluate a comma-separated list of expressions
//! against every record and write one output record per input record.
//!
//! ```sh
//! ndjson select 'd.name, d.price * d.qty as total, toupper(d.state) as state'
//! ndjson select '*, d.price * d.qty as total'
//! ndjson select '* except (d.ssn, d.internal)'
//! ndjson select -r 'd.person.gender' | sort | uniq -c
//! ndjson select 'count() as n, sum(d.amount), max(d.amount), d.name'
//! ndjson sort -k d.state | ndjson select 'd.state, count(), avg(d.salary)' --by d.state
//! ```
//!
//! The list is parsed by rmath-ast's `SelectList` rule, so expressions are
//! the same language `filter` uses. See `plans/02-select-and-expressions.md`
//! for output naming and the output modes.
//!
//! A list that calls an aggregate (`count()`, `sum(d.x)`), or a run with
//! `--by`, is in *aggregate mode*: it writes one record per group (see
//! [`crate::group`]) instead of one per input record. Without `--by` the
//! whole stream is one group.
//!
//! A missing path is `null`. An expression that fails to evaluate
//! (`d.name * 2`) is written as `null` and counted in the returned
//! [`SelectSummary`]; in aggregate mode the failing record is skipped by
//! that aggregate.

use crate::{
    group::{group_rows, AggregateSummary, Aggregation, Grouping},
    records::{for_each_record, write_record, Flow},
};
use ndjson_common::{error::NdJsonSpatialError, json_selector_parser::Selector};
use rmath_ast::{
    parser::{
        ast::{Expr, SelectItem, Term},
        math::SelectListParser,
    },
    AggregateList, CompiledExpr, EvalError, FunctionRegistry,
};
use serde_json::{Map, Value};
use std::{collections::HashMap, io::BufRead, io::Write};

/// How output records are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// One JSON object per record, keys in select-list order.
    Object,
    /// The bare JSON value of a single item, or a JSON array of the items.
    Raw,
    /// The items' values separated by tabs: strings unquoted, `null` empty.
    Tsv,
}

/// One parsed item of a select list, before function calls are resolved.
#[derive(Debug, PartialEq)]
pub enum ParsedItem {
    /// `*`, optionally `except (...)`.
    Star { except: Vec<Vec<Selector>> },
    Expr {
        /// The output key: the alias, or a name derived from the expression.
        name: String,
        /// The expression's source text, whitespace-normalised.
        text: String,
        expr: Expr,
    },
}

/// A parsed select list.
#[derive(Debug, PartialEq)]
pub struct SelectList {
    pub items: Vec<ParsedItem>,
    /// Items named after their whole text, which should get an `as` alias.
    pub warnings: Vec<String>,
}

/// Parse a select list and name its items. `*` may appear once. In
/// [`OutputMode::Object`] two items with the same output key are an error;
/// the other modes don't use keys.
pub fn parse_select_list(src: &str, mode: OutputMode) -> Result<SelectList, String> {
    let items = SelectListParser::new()
        .parse(src)
        .map_err(|e| format!("parse error: {}", e))?;

    let mut parsed = Vec::with_capacity(items.len());
    let mut warnings = vec![];
    let mut names: HashMap<String, String> = HashMap::new();
    let mut seen_star = false;
    for item in items {
        match item {
            SelectItem::Star { except } => {
                if seen_star {
                    return Err("'*' can only appear once".to_string());
                }
                seen_star = true;
                parsed.push(ParsedItem::Star { except });
            }
            SelectItem::Expr {
                expr,
                alias,
                span: (start, end),
            } => {
                let text = normalise_whitespace(&src[start..end]);
                let name = match alias {
                    Some(alias) => alias,
                    None => match default_name(&expr) {
                        Some(name) => name,
                        None => {
                            if mode == OutputMode::Object {
                                warnings.push(format!(
                                    "'{}' is named after its text; add 'as <name>' to rename it",
                                    text
                                ));
                            }
                            text.clone()
                        }
                    },
                };
                if mode == OutputMode::Object {
                    if let Some(other) = names.insert(name.clone(), text.clone()) {
                        return Err(format!(
                            "'{}' and '{}' both produce the key '{}'; rename one with 'as'",
                            other, text, name
                        ));
                    }
                }
                parsed.push(ParsedItem::Expr { name, text, expr });
            }
        }
    }
    Ok(SelectList {
        items: parsed,
        warnings,
    })
}

/// The key for an item without an alias, when the expression has a natural
/// one: `d.address.city` → `city`, `d.tags[0]` → `tags_0`,
/// `f(d.amount)` → `amount_f`, `f()` → `f`.
fn default_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Term(Term::Selector(path)) => selector_name(path),
        Expr::Call(function, args) => match args.as_slice() {
            [] => Some(function.clone()),
            [Expr::Term(Term::Selector(path))] => {
                selector_name(path).map(|name| format!("{}_{}", name, function))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The last identifier of `path` followed by any indices after it.
fn selector_name(path: &[Selector]) -> Option<String> {
    let last_ident = path
        .iter()
        .rposition(|s| matches!(s, Selector::Identifier(_)))?;
    let mut name = String::new();
    for selector in &path[last_ident..] {
        match selector {
            Selector::Identifier(ident) => name.push_str(ident),
            Selector::Index(i) => name.push_str(&format!("_{}", i)),
        }
    }
    Some(name)
}

/// Collapse runs of whitespace outside string literals to one space.
fn normalise_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut pending_space = false;
    for c in text.trim().chars() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
        } else if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            if c == '"' {
                in_string = true;
            }
            out.push(c);
        }
    }
    out
}

/// How aggregate mode groups records.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupOptions {
    /// The group key expressions (`--by`); empty groups the whole stream.
    pub by: Vec<Expr>,
    /// How the input is grouped by `by`.
    pub strategy: GroupStrategy,
    /// Allow aggregates whose memory grows with the input
    /// (`count_distinct`, `collect`).
    pub allow_unbounded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStrategy {
    /// The input is sorted by the key; out-of-order keys are an error.
    Sorted,
    /// Equal keys are adjacent, in any order. Not checked.
    Grouped,
    /// Any order; at most this many groups are kept in memory.
    Hash { max_groups: usize },
}

impl Default for GroupOptions {
    fn default() -> Self {
        GroupOptions {
            by: vec![],
            strategy: GroupStrategy::Sorted,
            allow_unbounded: false,
        }
    }
}

/// Parse a `--by` list: comma-separated expressions, without `as` or `*`.
pub fn parse_by(src: &str) -> Result<Vec<Expr>, String> {
    parse_key_list("--by", src)
}

/// Parse a comma-separated list of key expressions for `flag` (`--by`,
/// `--on`, ...): no `*` and no `as`.
pub fn parse_key_list(flag: &str, src: &str) -> Result<Vec<Expr>, String> {
    SelectListParser::new()
        .parse(src)
        .map_err(|e| format!("{}: parse error: {}", flag, e))?
        .into_iter()
        .map(|item| match item {
            SelectItem::Expr {
                expr, alias: None, ..
            } => Ok(expr),
            SelectItem::Expr { .. } => Err(format!("{}: keys can't be renamed with 'as'", flag)),
            SelectItem::Star { .. } => Err(format!("{}: '*' can't be a key", flag)),
        })
        .collect()
}

enum CompiledItem {
    Star { except: Vec<Vec<Selector>> },
    Expr { name: String, expr: CompiledExpr },
}

/// Evaluation failures for one item.
#[derive(Debug, PartialEq)]
pub struct ItemErrors {
    /// The item's text (`*` for the star item).
    pub item: String,
    pub count: u64,
    /// 1-based record number and the error of the first failure.
    pub first: (u64, EvalError),
}

/// What a select run couldn't evaluate, one entry per failing item in
/// select-list order.
#[derive(Debug, Default, PartialEq)]
pub struct SelectSummary {
    pub errors: Vec<ItemErrors>,
    /// Whether the list ran in aggregate mode, where a value that fails to
    /// evaluate is skipped by its aggregate rather than written as `null`.
    pub aggregate: bool,
}

/// Compile `list` and write the selected values of every record of `read`
/// to `write`. Stops quietly when the reader of `write` goes away.
pub fn ndjson_select<R: BufRead, W: Write>(
    list: &SelectList,
    mode: OutputMode,
    read: &mut R,
    write: &mut W,
) -> Result<SelectSummary, NdJsonSpatialError> {
    ndjson_select_grouped(list, mode, &GroupOptions::default(), read, write)
}

/// [`ndjson_select`], grouping by `options` when the list has aggregates
/// or `options.by` isn't empty.
pub fn ndjson_select_grouped<R: BufRead, W: Write>(
    list: &SelectList,
    mode: OutputMode,
    options: &GroupOptions,
    read: &mut R,
    write: &mut W,
) -> Result<SelectSummary, NdJsonSpatialError> {
    let registry = FunctionRegistry::default();
    let is_aggregate = !options.by.is_empty()
        || list.items.iter().any(|item| match item {
            ParsedItem::Expr { expr, .. } => registry.contains_aggregate(expr),
            ParsedItem::Star { .. } => false,
        });
    if is_aggregate {
        return select_aggregate(&registry, list, mode, options, read, write);
    }

    let items = list
        .items
        .iter()
        .map(|item| match item {
            ParsedItem::Star { except } => Ok(CompiledItem::Star {
                except: except.clone(),
            }),
            ParsedItem::Expr { name, text, expr } => registry
                .resolve(expr)
                .map(|expr| CompiledItem::Expr {
                    name: name.clone(),
                    expr,
                })
                .map_err(|e| NdJsonSpatialError::Error(format!("in '{}': {}", text, e))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let texts: Vec<&str> = list
        .items
        .iter()
        .map(|item| match item {
            ParsedItem::Star { .. } => "*",
            ParsedItem::Expr { text, .. } => text.as_str(),
        })
        .collect();

    let mut errors: Vec<Option<(u64, (u64, EvalError))>> = vec![None; items.len()];
    let mut record_number = 0;
    let mut line = vec![];
    for_each_record(read, |record| {
        record_number += 1;
        let record: Value = serde_json::from_slice(record).map_err(|e| {
            NdJsonSpatialError::Error(format!("record {} is not JSON: {}", record_number, e))
        })?;
        let mut failed = |i: usize, e: EvalError| match &mut errors[i] {
            Some((count, _)) => *count += 1,
            slot => *slot = Some((1, (record_number, e))),
        };
        let row = select_row(&items, record, mode, &mut failed);

        line.clear();
        write_row(&mut line, row, mode);
        Ok(if write_record(write, &line)? {
            Flow::Continue
        } else {
            Flow::Stop
        })
    })?;
    crate::head::flush(write)?;

    Ok(SelectSummary {
        aggregate: false,
        errors: errors
            .into_iter()
            .zip(texts)
            .filter_map(|(e, text)| {
                e.map(|(count, first)| ItemErrors {
                    item: text.to_string(),
                    count,
                    first,
                })
            })
            .collect(),
    })
}

fn select_aggregate<R: BufRead, W: Write>(
    registry: &FunctionRegistry,
    list: &SelectList,
    mode: OutputMode,
    options: &GroupOptions,
    read: &mut R,
    write: &mut W,
) -> Result<SelectSummary, NdJsonSpatialError> {
    let error = |e: String| NdJsonSpatialError::Error(e);
    let mut names = vec![];
    let mut texts = vec![];
    let mut exprs = vec![];
    for item in &list.items {
        match item {
            ParsedItem::Star { .. } => {
                return Err(error(
                    "'*' can't be used with aggregates or --by".to_string(),
                ))
            }
            ParsedItem::Expr { name, text, expr } => {
                names.push(name.clone());
                texts.push(text.as_str());
                exprs.push(expr.clone());
            }
        }
    }

    let by = options
        .by
        .iter()
        .map(|key| registry.resolve(key))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(format!("in --by: {}", e)))?;
    let aggregates = AggregateList::compile(registry, &exprs, &options.by).map_err(|e| {
        error(match e {
            rmath_ast::ResolveError::Bare { item } => format!(
                "in '{}': a field outside an aggregate must be a --by key, or the list must \
                 have exactly one min() or max() to take it from; use first(), last(), \
                 argmax() or argmin(), or add it to --by",
                texts[item - 1]
            ),
            e => e.to_string(),
        })
    })?;
    let unbounded = aggregates.unbounded_functions();
    if !unbounded.is_empty() && !options.allow_unbounded {
        return Err(error(format!(
            "{} use{} memory that grows with the input; pass --allow-unbounded",
            unbounded.join(", "),
            if unbounded.len() == 1 { "s" } else { "" }
        )));
    }

    let grouping = match (options.by.is_empty(), options.strategy) {
        (true, _) => Grouping::Stream,
        (false, GroupStrategy::Sorted) => Grouping::Sorted { check_order: true },
        (false, GroupStrategy::Grouped) => Grouping::Sorted { check_order: false },
        (false, GroupStrategy::Hash { max_groups }) => Grouping::Hash { max_groups },
    };
    let aggregation = Aggregation {
        names,
        list: aggregates,
        by,
        grouping,
    };

    let mut line = vec![];
    let summary = group_rows(&aggregation, read, |values| {
        let row = match mode {
            OutputMode::Object => {
                Row::Object(aggregation.names.iter().cloned().zip(values).collect())
            }
            OutputMode::Raw | OutputMode::Tsv => Row::Values(values),
        };
        line.clear();
        write_row(&mut line, row, mode);
        write_record(write, &line)
    })
    .map_err(|NdJsonSpatialError::Error(e)| {
        let hint = if e.starts_with("more than") {
            " (--max-groups)"
        } else if e.contains("must be sorted") {
            "; sort it first, or use --hash (or --grouped if equal keys are adjacent)"
        } else {
            ""
        };
        error(format!("{}{}", e.replace("group key", "--by key"), hint))
    })?;
    crate::head::flush(write)?;

    let AggregateSummary { items, key, .. } = summary;
    Ok(SelectSummary {
        aggregate: true,
        errors: items
            .into_iter()
            .zip(texts)
            .filter_map(|(e, text)| e.map(|e| (text.to_string(), e)))
            .chain(key.map(|e| ("--by".to_string(), e)))
            .map(|(item, (count, first))| ItemErrors { item, count, first })
            .collect(),
    })
}

/// A record's output: key/value pairs for [`OutputMode::Object`], the
/// items' values otherwise.
enum Row {
    Object(Map<String, Value>),
    Values(Vec<Value>),
}

fn select_row(
    items: &[CompiledItem],
    mut record: Value,
    mode: OutputMode,
    failed: &mut impl FnMut(usize, EvalError),
) -> Row {
    // Expressions see the whole record, including fields `* except` drops.
    let values: Vec<Option<Value>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| match item {
            CompiledItem::Star { .. } => None,
            CompiledItem::Expr { expr, .. } => Some(expr.eval(&record).unwrap_or_else(|e| {
                failed(i, e);
                Value::Null
            })),
        })
        .collect();

    for item in items {
        if let CompiledItem::Star { except } = item {
            for path in except {
                remove_path(&mut record, path);
            }
        }
    }

    match mode {
        OutputMode::Object => {
            let mut out = Map::new();
            let mut record = Some(record);
            for (i, (item, value)) in items.iter().zip(values).enumerate() {
                match (item, value) {
                    (CompiledItem::Expr { name, .. }, Some(value)) => {
                        // A later item with an existing key replaces its value
                        // and keeps its position.
                        out.insert(name.clone(), value);
                    }
                    _ => match record.take() {
                        Some(Value::Object(fields)) => {
                            for (k, v) in fields {
                                out.insert(k, v);
                            }
                        }
                        Some(other) => failed(
                            i,
                            EvalError::Type {
                                op: "*",
                                left: type_name(&other),
                                right: None,
                            },
                        ),
                        None => {}
                    },
                }
            }
            Row::Object(out)
        }
        OutputMode::Raw | OutputMode::Tsv => {
            let mut record = Some(record);
            Row::Values(
                values
                    .into_iter()
                    .map(|v| v.unwrap_or_else(|| record.take().unwrap_or(Value::Null)))
                    .collect(),
            )
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Remove the value at `path` from `value`, if it's there.
fn remove_path(value: &mut Value, path: &[Selector]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = value;
    for selector in parents {
        let next = match (selector, current) {
            (Selector::Identifier(key), Value::Object(fields)) => fields.get_mut(key),
            (Selector::Index(i), Value::Array(items)) => items.get_mut(*i),
            _ => None,
        };
        match next {
            Some(next) => current = next,
            None => return,
        }
    }
    match (last, current) {
        (Selector::Identifier(key), Value::Object(fields)) => {
            fields.shift_remove(key);
        }
        (Selector::Index(i), Value::Array(items)) if *i < items.len() => {
            items.remove(*i);
        }
        _ => {}
    }
}

fn write_row(line: &mut Vec<u8>, row: Row, mode: OutputMode) {
    let to_vec = |v: &Value| serde_json::to_vec(v).expect("JSON values serialize");
    match (row, mode) {
        (Row::Object(fields), _) => line.extend(to_vec(&Value::Object(fields))),
        (Row::Values(mut values), OutputMode::Raw) => {
            if values.len() == 1 {
                line.extend(to_vec(&values.remove(0)));
            } else {
                line.extend(to_vec(&Value::Array(values)));
            }
        }
        (Row::Values(values), _) => {
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    line.push(b'\t');
                }
                match value {
                    Value::Null => {}
                    Value::String(s) => tsv_escape(line, s),
                    other => line.extend(to_vec(other)),
                }
            }
        }
    }
}

/// Strings in TSV output escape `\`, tab, newline and carriage return as
/// `\\`, `\t`, `\n` and `\r`, so each record stays one line of
/// tab-separated fields.
fn tsv_escape(line: &mut Vec<u8>, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => line.extend(b"\\\\"),
            '\t' => line.extend(b"\\t"),
            '\n' => line.extend(b"\\n"),
            '\r' => line.extend(b"\\r"),
            c => {
                let mut buf = [0; 4];
                line.extend(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEOPLE: &str = concat!(
        "{\"name\":\"Ada\",\"address\":{\"city\":\"London\",\"zip\":\"N1\"},\"price\":3,\"qty\":4,\"tags\":[\"x\",\"y\"]}\n",
        "{\"name\":\"Grace\",\"price\":\"ten\",\"qty\":1}\n",
    );

    fn run(list: &str, mode: OutputMode, input: &str) -> (String, SelectSummary) {
        let list = parse_select_list(list, mode).unwrap();
        let mut output = vec![];
        let summary = ndjson_select(&list, mode, &mut input.as_bytes(), &mut output).unwrap();
        (String::from_utf8(output).unwrap(), summary)
    }

    fn select(list: &str, input: &str) -> String {
        run(list, OutputMode::Object, input).0
    }

    #[test]
    fn test_projection_keeps_list_order() {
        assert_eq!(
            select("d.qty, d.address.city, d.name", PEOPLE),
            concat!(
                "{\"qty\":4,\"city\":\"London\",\"name\":\"Ada\"}\n",
                "{\"qty\":1,\"city\":null,\"name\":\"Grace\"}\n",
            )
        );
    }

    #[test]
    fn test_computed_fields_and_aliases() {
        let (output, summary) = run(
            "d.name, d.price * d.qty as total, toupper(d.name) as \"upper name\"",
            OutputMode::Object,
            PEOPLE,
        );
        assert_eq!(
            output,
            concat!(
                "{\"name\":\"Ada\",\"total\":12,\"upper name\":\"ADA\"}\n",
                "{\"name\":\"Grace\",\"total\":null,\"upper name\":\"GRACE\"}\n",
            )
        );
        assert_eq!(
            summary.errors,
            vec![ItemErrors {
                item: "d.price * d.qty".into(),
                count: 1,
                first: (
                    2,
                    EvalError::Type {
                        op: "*",
                        left: "string",
                        right: Some("number")
                    }
                ),
            }]
        );
    }

    #[test]
    fn test_star() {
        assert_eq!(
            select("*, d.price * d.qty as total", "{\"price\":3,\"qty\":4}\n"),
            "{\"price\":3,\"qty\":4,\"total\":12}\n"
        );
        // a later item with the same key as a record field replaces it in
        // place; an earlier one is replaced by the record's value
        assert_eq!(
            select(
                "toupper(d.name) as name, *",
                "{\"id\":1,\"name\":\"ada\",\"x\":2}\n"
            ),
            "{\"name\":\"ada\",\"id\":1,\"x\":2}\n"
        );
        assert_eq!(
            select(
                "*, toupper(d.name) as name",
                "{\"id\":1,\"name\":\"ada\",\"x\":2}\n"
            ),
            "{\"id\":1,\"name\":\"ADA\",\"x\":2}\n"
        );
    }

    #[test]
    fn test_star_except() {
        assert_eq!(
            select("* except (d.address.zip, d.tags, d.missing.x)", PEOPLE),
            concat!(
                "{\"name\":\"Ada\",\"address\":{\"city\":\"London\"},\"price\":3,\"qty\":4}\n",
                "{\"name\":\"Grace\",\"price\":\"ten\",\"qty\":1}\n",
            )
        );
        // expressions still see the dropped fields
        assert_eq!(
            select(
                "* except (d.tags), d.tags[1]",
                "{\"a\":1,\"tags\":[\"x\",\"y\"]}\n"
            ),
            "{\"a\":1,\"tags_1\":\"y\"}\n"
        );
        assert_eq!(
            select("* except (d.a[0])", "{\"a\":[1,2,3]}\n"),
            "{\"a\":[2,3]}\n"
        );
    }

    #[test]
    fn test_star_on_a_non_object_is_an_error() {
        let (output, summary) = run("*, d[0] as first", OutputMode::Object, "[1,2]\n");
        assert_eq!(output, "{\"first\":1}\n");
        assert_eq!(summary.errors[0].item, "*");
        assert_eq!(summary.errors[0].count, 1);
    }

    #[test]
    fn test_output_names() {
        let names = |list: &str| -> Vec<String> {
            parse_select_list(list, OutputMode::Object)
                .unwrap()
                .items
                .into_iter()
                .filter_map(|item| match item {
                    ParsedItem::Expr { name, .. } => Some(name),
                    ParsedItem::Star { .. } => None,
                })
                .collect()
        };
        assert_eq!(names("d.address.city"), ["city"]);
        assert_eq!(names("d.tags[0], d.a[1][2]"), ["tags_0", "a_1_2"]);
        assert_eq!(names(r#"d."key with spaces""#), ["key with spaces"]);
        assert_eq!(names("abs(d.amount), pi()"), ["amount_abs", "pi"]);
        assert_eq!(
            names("d.price  *\td.qty, round(d.x, 2), d[0], \"a  b\""),
            ["d.price * d.qty", "round(d.x, 2)", "d[0]", "\"a  b\""]
        );
        assert_eq!(names("d.a + 1 as b"), ["b"]);
    }

    #[test]
    fn test_unaliased_text_names_warn() {
        let list = parse_select_list("d.a, d.a + 1, d.b as b", OutputMode::Object).unwrap();
        assert_eq!(list.warnings.len(), 1);
        assert!(list.warnings[0].contains("'d.a + 1'"));
        // raw and tsv output have no keys to warn about
        let list = parse_select_list("d.a + 1", OutputMode::Raw).unwrap();
        assert!(list.warnings.is_empty());
    }

    #[test]
    fn test_duplicate_keys_are_an_error() {
        assert!(parse_select_list("d.a.x, d.b.x", OutputMode::Object).is_err());
        assert!(parse_select_list("d.a, d.b as a", OutputMode::Object).is_err());
        assert!(parse_select_list("d.a.x, d.b.x as y", OutputMode::Object).is_ok());
        // keys don't matter for raw and tsv output
        assert!(parse_select_list("d.a.x, d.b.x", OutputMode::Raw).is_ok());
        assert!(parse_select_list("d.a.x, d.b.x", OutputMode::Tsv).is_ok());
    }

    #[test]
    fn test_star_appears_once() {
        assert!(parse_select_list("*, *", OutputMode::Object).is_err());
    }

    #[test]
    fn test_unknown_functions_are_an_error() {
        let list = parse_select_list("nope(d.a)", OutputMode::Object).unwrap();
        let mut output = vec![];
        assert!(ndjson_select(
            &list,
            OutputMode::Object,
            &mut "{}\n".as_bytes(),
            &mut output
        )
        .is_err());
    }

    #[test]
    fn test_raw() {
        assert_eq!(
            run("d.name", OutputMode::Raw, PEOPLE).0,
            "\"Ada\"\n\"Grace\"\n"
        );
        assert_eq!(
            run("d.name, d.address.city", OutputMode::Raw, PEOPLE).0,
            "[\"Ada\",\"London\"]\n[\"Grace\",null]\n"
        );
        assert_eq!(
            run("* except (d.b)", OutputMode::Raw, "{\"a\":1,\"b\":2}\n").0,
            "{\"a\":1}\n"
        );
    }

    #[test]
    fn test_tsv() {
        assert_eq!(
            run(
                "d.name, d.qty, d.address, d.address.zip",
                OutputMode::Tsv,
                PEOPLE
            )
            .0,
            concat!(
                "Ada\t4\t{\"city\":\"London\",\"zip\":\"N1\"}\tN1\n",
                "Grace\t1\t\t\n",
            )
        );
        assert_eq!(
            run("d.s", OutputMode::Tsv, "{\"s\":\"a\\tb\\nc\\\\d\"}\n").0,
            "a\\tb\\nc\\\\d\n"
        );
    }

    #[test]
    fn test_skips_blank_lines_and_rejects_bad_json() {
        assert_eq!(
            select("d.a", "{\"a\":1}\n\n{\"a\":2}\n"),
            "{\"a\":1}\n{\"a\":2}\n"
        );
        let list = parse_select_list("d.a", OutputMode::Object).unwrap();
        let mut output = vec![];
        assert!(ndjson_select(
            &list,
            OutputMode::Object,
            &mut "{\"a\":1}\nnope\n".as_bytes(),
            &mut output
        )
        .is_err());
    }

    const SALES: &str = concat!(
        "{\"state\":\"IA\",\"amount\":5,\"name\":\"Ada\"}\n",
        "{\"state\":\"MN\",\"amount\":2,\"name\":\"Grace\"}\n",
        "{\"state\":\"MN\",\"amount\":\"n/a\",\"name\":\"Alan\"}\n",
        "{\"state\":\"MN\",\"amount\":9,\"name\":\"Edsger\"}\n",
    );

    fn grouped(
        list: &str,
        mode: OutputMode,
        by: &str,
        strategy: GroupStrategy,
        input: &str,
    ) -> Result<(String, SelectSummary), NdJsonSpatialError> {
        let list = parse_select_list(list, mode).unwrap();
        let options = GroupOptions {
            by: if by.is_empty() {
                vec![]
            } else {
                parse_by(by).unwrap()
            },
            strategy,
            allow_unbounded: false,
        };
        let mut output = vec![];
        let summary =
            ndjson_select_grouped(&list, mode, &options, &mut input.as_bytes(), &mut output)?;
        Ok((String::from_utf8(output).unwrap(), summary))
    }

    #[test]
    fn test_whole_stream_aggregates() {
        let (output, summary) = grouped(
            "count() as n, sum(d.amount), max(d.amount), d.name",
            OutputMode::Object,
            "",
            GroupStrategy::Sorted,
            SALES,
        )
        .unwrap();
        // the bare d.name comes from the row holding the max
        assert_eq!(
            output,
            "{\"n\":4,\"amount_sum\":16,\"amount_max\":9,\"name\":\"Edsger\"}\n"
        );
        assert!(summary.aggregate);
        let items: Vec<_> = summary.errors.iter().map(|e| e.item.as_str()).collect();
        assert_eq!(items, ["sum(d.amount)", "max(d.amount)", "d.name"]);
        assert_eq!(summary.errors[0].first.0, 3);

        // empty input still has one row
        assert_eq!(
            grouped(
                "count(), sum(d.x)",
                OutputMode::Object,
                "",
                GroupStrategy::Sorted,
                ""
            )
            .unwrap()
            .0,
            "{\"count\":0,\"x_sum\":null}\n"
        );
    }

    #[test]
    fn test_grouped_aggregates() {
        let expected = concat!(
            "{\"state\":\"IA\",\"count\":1,\"total\":5}\n",
            "{\"state\":\"MN\",\"count\":3,\"total\":11}\n",
        );
        for strategy in [
            GroupStrategy::Sorted,
            GroupStrategy::Grouped,
            GroupStrategy::Hash { max_groups: 10 },
        ] {
            let (output, _) = grouped(
                "d.state, count(), sum(d.amount) as total",
                OutputMode::Object,
                "d.state",
                strategy,
                SALES,
            )
            .unwrap();
            assert_eq!(output, expected, "{:?}", strategy);
        }
        assert_eq!(
            grouped(
                "d.state, count()",
                OutputMode::Tsv,
                "d.state",
                GroupStrategy::Sorted,
                SALES
            )
            .unwrap()
            .0,
            "IA\t1\nMN\t3\n"
        );
        assert_eq!(
            grouped(
                "count()",
                OutputMode::Raw,
                "d.state",
                GroupStrategy::Sorted,
                SALES
            )
            .unwrap()
            .0,
            "1\n3\n"
        );
    }

    #[test]
    fn test_unsorted_by_is_an_error() {
        let reversed: String = SALES.lines().rev().map(|l| format!("{}\n", l)).collect();
        let err = grouped(
            "d.state, count()",
            OutputMode::Object,
            "d.state",
            GroupStrategy::Sorted,
            &reversed,
        )
        .unwrap_err();
        assert_eq!(
            err,
            NdJsonSpatialError::Error(
                "record 4: --by key \"IA\" sorts before the previous key \"MN\"; the input must \
                 be sorted by the --by key; sort it first, or use --hash (or --grouped if equal \
                 keys are adjacent)"
                    .into()
            )
        );
        let (output, _) = grouped(
            "d.state, count()",
            OutputMode::Object,
            "d.state",
            GroupStrategy::Hash { max_groups: 10 },
            &reversed,
        )
        .unwrap();
        assert_eq!(
            output,
            "{\"state\":\"IA\",\"count\":1}\n{\"state\":\"MN\",\"count\":3}\n"
        );
    }

    #[test]
    fn test_aggregate_mode_errors() {
        let err = |list: &str, by: &str| match grouped(
            list,
            OutputMode::Object,
            by,
            GroupStrategy::Sorted,
            SALES,
        ) {
            Err(NdJsonSpatialError::Error(e)) => e,
            Ok(_) => panic!("{} succeeded", list),
        };
        assert!(err("d.name, count()", "d.state").starts_with("in 'd.name': a field outside"));
        assert!(err("sum(d.amount), d.name", "").starts_with("in 'd.name'"));
        assert_eq!(
            err("*, count()", ""),
            "'*' can't be used with aggregates or --by"
        );
        assert_eq!(
            err("collect(d.name)", ""),
            "collect uses memory that grows with the input; pass --allow-unbounded"
        );
        assert_eq!(
            err("sum(max(d.amount))", ""),
            "max() can't be used inside the aggregate sum()"
        );
        assert_eq!(
            err("d.state", "count()"),
            "in --by: count() is an aggregate function and can't be used here"
        );
        assert!(parse_by("d.a as b").is_err());
        assert!(parse_by("*").is_err());
        assert_eq!(parse_by("d.a, floor(d.b / 2)").unwrap().len(), 2);
    }
}
