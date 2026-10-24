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

//! `ndjson filter <expr>`: keep the records for which an rmath-ast
//! expression is `true`.
//!
//! The expression language is shared with `select` (see
//! `plans/02-select-and-expressions.md`): comparisons, `and`/`or`/`not`,
//! `in [..]`, `is null`, arithmetic, and function calls. Records where the
//! expression is `false` or `null` are dropped; so are records where
//! evaluation fails (`d.s * 2` on a string), which are counted in the
//! returned [`FilterSummary`].
//!
//! The old `selector <op> value` syntax keeps working, including unquoted
//! strings (`d.product == Virtual Currency`) and keys the new lexer
//! doesn't accept unquoted (`d.foo-bar > 1`).

use ndjson_common::{
    error::NdJsonSpatialError,
    json_selector_parser::{parse_json_selector, Selector},
    ndjson::NdjsonReader,
};
use rmath_ast::{
    parser::{
        ast::{CompareOp, Expr, Term, Value as Literal},
        math::SelectorParser,
        ExprParser,
    },
    CompileError, CompiledExpr, EvalError, FunctionRegistry,
};
use serde_json::Value;
use std::io::{BufRead, BufReader, BufWriter, Write};

/// What a filter run skipped because the expression failed to evaluate.
#[derive(Debug, Default, PartialEq)]
pub struct FilterSummary {
    /// Records dropped because evaluating the expression failed.
    pub errors: u64,
    /// The first failure: 1-based record number and the error.
    pub first_error: Option<(u64, EvalError)>,
}

pub fn ndjson_filter<R: BufRead, W: Write>(
    expression: String,
    read: &mut R,
    write: &mut W,
) -> Result<FilterSummary, NdJsonSpatialError> {
    ndjson_filter_with(&FunctionRegistry::default(), &expression, read, write)
}

/// [`ndjson_filter`] with the functions in `registry`.
pub fn ndjson_filter_with<R: BufRead, W: Write>(
    registry: &FunctionRegistry,
    expression: &str,
    read: &mut R,
    write: &mut W,
) -> Result<FilterSummary, NdJsonSpatialError> {
    let predicate = compile_filter(registry, expression).map_err(|e| {
        NdJsonSpatialError::Error(format!("invalid filter {:?}: {}", expression, e))
    })?;

    let read = BufReader::with_capacity(1_000_000, read);
    let mut write = BufWriter::with_capacity(1_000_000, write);
    let mut summary = FilterSummary::default();

    for (n, value) in (1..).zip(NdjsonReader::new(read)) {
        let v = value?;
        match predicate.eval_bool(&v) {
            Ok(true) => writeln!(write, "{}", v).expect("unable to write to stdout"),
            Ok(false) => {}
            Err(e) => {
                summary.errors += 1;
                summary.first_error.get_or_insert((n, e));
            }
        }
    }
    Ok(summary)
}

/// Compile a filter expression, falling back to the old
/// `selector <op> value` syntax when it isn't a valid expression.
pub fn compile_filter(
    registry: &FunctionRegistry,
    expression: &str,
) -> Result<CompiledExpr, CompileError> {
    match registry.compile(expression) {
        Ok(compiled) => Ok(compiled),
        Err(err @ CompileError::Parse(_)) => match legacy_expression(expression) {
            Some(expr) => Ok(registry.resolve(&expr)?),
            None => Err(err),
        },
        Err(err) => Err(err),
    }
}

/// Parse the pre-rmath-ast filter syntax: a selector, a comparator, and a
/// value that is a literal or, failing that, an unquoted string.
fn legacy_expression(expression: &str) -> Option<Expr> {
    let split = expression.find(['<', '>', '=', '!'])?;
    let (selector, rest) = expression.split_at(split);
    let selector = selector.trim();

    let (op, value) = [
        ("==", CompareOp::Eq),
        ("!=", CompareOp::Ne),
        ("<=", CompareOp::Le),
        (">=", CompareOp::Ge),
        ("<", CompareOp::Lt),
        (">", CompareOp::Gt),
    ]
    .iter()
    .find_map(|&(text, op)| rest.strip_prefix(text).map(|value| (op, value.trim())))?;
    if value.is_empty() {
        return None;
    }

    let path = legacy_selector(selector)?;
    let value = match ExprParser::new().parse(value) {
        Ok(value) => value,
        // An unquoted string, unless it looks like a mistyped expression
        // (`d.a == 1 and d.b = 2`), which should stay a parse error.
        Err(_) if !value.contains(['<', '>', '=', '!', '"', '(', ')', '[', ']']) => {
            Box::new(Expr::Term(Term::Value(Literal::String(value.to_string()))))
        }
        Err(_) => return None,
    };
    Some(Expr::Compare(
        Box::new(Expr::Term(Term::Selector(path))),
        op,
        value,
    ))
}

pub(crate) fn legacy_selector(selector: &str) -> Option<Vec<Selector>> {
    if let Ok(path) = SelectorParser::new().parse(selector) {
        return Some(path);
    }
    match parse_json_selector(selector.into()) {
        Ok((rest, path)) if rest.is_empty() => Some(
            path.into_iter()
                .map(|s| match s {
                    // the old parser keeps identifiers wrapped in quotes
                    Selector::Identifier(ident) => Selector::Identifier(
                        ident
                            .strip_prefix('"')
                            .and_then(|s| s.strip_suffix('"'))
                            .unwrap_or(&ident)
                            .to_string(),
                    ),
                    index => index,
                })
                .collect(),
        ),
        _ => None,
    }
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
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(ident.as_str());

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
                        ident
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

    fn filter(expression: &str, input: &str) -> String {
        let mut output = vec![];
        ndjson_filter(expression.to_string(), &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_filter_i64() {
        let input = "{ \"a\": 1 }\n{ \"a\": -45 }\n";
        assert_eq!(filter("d.a < -40", input), "{\"a\":-45}\n");
        assert_eq!(filter("d.a > -40", input), "{\"a\":1}\n");

        let input = "{ \"a\": 40250 }\n{ \"a\": -45 }\n";
        assert_eq!(filter("d.a > 10000", input), "{\"a\":40250}\n");
        assert_eq!(filter("d.a < 10000", input), "{\"a\":-45}\n");
    }

    #[test]
    fn test_filter_bool() {
        let input = "{ \"a\": true, \"b\": \"foo\" }\n{ \"a\": false, \"b\": \"bar\" }\n";
        assert_eq!(filter("d.a == true", input), "{\"a\":true,\"b\":\"foo\"}\n");
        assert_eq!(
            filter("d.a == false", input),
            "{\"a\":false,\"b\":\"bar\"}\n"
        );
        // a bool field on its own is a predicate
        assert_eq!(filter("d.a", input), "{\"a\":true,\"b\":\"foo\"}\n");
        assert_eq!(filter("not d.a", input), "{\"a\":false,\"b\":\"bar\"}\n");
    }

    #[test]
    fn test_filter_float() {
        assert_eq!(
            filter("d.a < 10.4", "{ \"a\": 10.4 }\n{ \"a\": -34.58 }\n"),
            "{\"a\":-34.58}\n"
        );
        assert_eq!(
            filter("d.a > 30.0", "{\"a\": 24 }\n{ \"a\": 54 }\n"),
            "{\"a\":54}\n"
        );
        // ints and floats compare by value
        assert_eq!(
            filter("d.a == 54", "{\"a\": 54.0 }\n{ \"a\": 54.5 }\n"),
            "{\"a\":54.0}\n"
        );
    }

    #[test]
    fn test_filter_null() {
        let input = "{ \"a\": null }\n{ \"a\": false }\n";
        assert_eq!(filter("d.a == null", input), "{\"a\":null}\n");
        assert_eq!(filter("d.a != null", input), "{\"a\":false}\n");
        assert_eq!(filter("d.a is null", input), "{\"a\":null}\n");
        assert_eq!(filter("d.a is not null", input), "{\"a\":false}\n");
        // a missing field is null
        assert_eq!(filter("d.b is null", "{\"a\":1}\n"), "{\"a\":1}\n");
    }

    const PEOPLE: &str = concat!(
        "{\"name\":\"Ada\",\"state\":\"MN\",\"price\":3,\"qty\":4}\n",
        "{\"name\":\"Grace\",\"state\":\"WI\",\"price\":10,\"qty\":1}\n",
        "{\"name\":\"Alan\",\"state\":\"IA\",\"price\":2,\"qty\":2}\n",
    );

    fn names(output: &str) -> Vec<String> {
        output
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["name"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn test_filter_boolean_operators() {
        assert_eq!(
            names(&filter(r#"d.state == "MN" or d.state == "IA""#, PEOPLE)),
            ["Ada", "Alan"]
        );
        assert_eq!(
            names(&filter(r#"d.price > 2 and not d.state == "WI""#, PEOPLE)),
            ["Ada"]
        );
        assert_eq!(
            names(&filter(r#"d.state in ["WI", "IA"]"#, PEOPLE)),
            ["Grace", "Alan"]
        );
        assert_eq!(
            names(&filter(r#"d.state not in ["WI", "IA"]"#, PEOPLE)),
            ["Ada"]
        );
    }

    #[test]
    fn test_filter_computed_comparisons_and_functions() {
        assert_eq!(names(&filter("d.price * d.qty > 10", PEOPLE)), ["Ada"]);
        assert_eq!(
            names(&filter("d.price * d.qty == d.price + d.qty", PEOPLE)),
            ["Alan"]
        );
        assert_eq!(
            names(&filter(r#"tolower(d.state) == "wi""#, PEOPLE)),
            ["Grace"]
        );
    }

    #[test]
    fn test_filter_legacy_unquoted_strings() {
        let input = concat!(
            "{\"product\":\"Virtual Currency\"}\n",
            "{\"product\":\"Mortgage\"}\n",
        );
        assert_eq!(
            filter("d.product == Virtual Currency", input),
            "{\"product\":\"Virtual Currency\"}\n"
        );
        assert_eq!(
            filter("d.product != Virtual Currency", input),
            "{\"product\":\"Mortgage\"}\n"
        );
        assert_eq!(names(&filter("d.name==Grace", PEOPLE)), ["Grace"]);
        assert_eq!(names(&filter("d.state >= MN", PEOPLE)), ["Ada", "Grace"]);
    }

    #[test]
    fn test_filter_legacy_keys() {
        let input = "{\"foo-bar\":5}\n{\"foo-bar\":1}\n";
        assert_eq!(filter("d.foo-bar > 2", input), "{\"foo-bar\":5}\n");
        assert_eq!(filter(r#"d."foo-bar" > 2"#, input), "{\"foo-bar\":5}\n");
    }

    #[test]
    fn test_filter_counts_eval_errors() {
        let input = "{\"a\":1}\n{\"a\":\"x\"}\n{\"a\":\"y\"}\n{\"a\":3}\n";
        let mut output = vec![];
        let summary = ndjson_filter(
            "d.a * 2 > 3".to_string(),
            &mut input.as_bytes(),
            &mut output,
        )
        .unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "{\"a\":3}\n");
        assert_eq!(summary.errors, 2);
        assert_eq!(
            summary.first_error,
            Some((
                2,
                EvalError::Type {
                    op: "*",
                    left: "string",
                    right: Some("number")
                }
            ))
        );
    }

    #[test]
    fn test_filter_non_bool_result_is_an_error() {
        let mut output = vec![];
        let summary = ndjson_filter(
            "d.a + 1".to_string(),
            &mut "{\"a\":1}\n".as_bytes(),
            &mut output,
        )
        .unwrap();
        assert!(output.is_empty());
        assert_eq!(
            summary.first_error,
            Some((1, EvalError::NotBool { got: "number" }))
        );
    }

    #[test]
    fn test_filter_rejects_bad_expressions() {
        for bad in [
            "d.a >",
            "d.a and",
            "nope(d.a) == 1",
            "abs(1, 2) > 0",
            "1 < d.a < 3",
            "d.a == 1 and d.b = 2",
        ] {
            let mut output = vec![];
            assert!(
                ndjson_filter(bad.to_string(), &mut "{}\n".as_bytes(), &mut output).is_err(),
                "{}",
                bad
            );
        }
    }
}
