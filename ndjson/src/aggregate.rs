//! `ndjson agg`: aggregates per group key, on top of [`crate::group`].
//!
//! `-g d.state -a sum d.pop -a max d.pop` is `select 'd.state, sum(d.pop),
//! max(d.pop)' --by d.state --hash`. Output fields are named after the
//! selectors: the key `d.address.state` is `address_state`, and
//! `-a sum d.pop` is `pop_sum`. `-a count <selector>` counts records, as it
//! always has.

use crate::{
    filter::legacy_selector,
    group::{group, AggregateSummary, Aggregation, Grouping},
};
use ndjson_common::{error::NdJsonSpatialError, json_selector_parser::Selector};
use rmath_ast::{
    parser::ast::{Expr, Term},
    AggregateList, FunctionRegistry,
};
use std::io::{BufRead, Write};

/// What `agg` computes and how it groups.
pub struct AggOptions {
    /// Selectors of the group key fields.
    pub group_by: Vec<String>,
    /// `(function, selector)` pairs.
    pub aggregates: Vec<(String, String)>,
    pub grouping: Grouping,
    /// Allow aggregates whose memory grows with the input.
    pub allow_unbounded: bool,
}

pub fn aggregate<I: BufRead, O: Write>(
    options: &AggOptions,
    input: &mut I,
    mut output: O,
) -> Result<AggregateSummary, NdJsonSpatialError> {
    let registry = FunctionRegistry::default();

    let mut names = Vec::new();
    let mut items = Vec::new();
    let mut by = Vec::new();
    for selector in &options.group_by {
        let path = parse_selector(selector)?;
        names.push(field_name(&path));
        items.push(Expr::Term(Term::Selector(path.clone())));
        by.push(Expr::Term(Term::Selector(path)));
    }
    for (function, selector) in &options.aggregates {
        let path = parse_selector(selector)?;
        if !registry.get(function).is_some_and(|f| f.is_aggregate()) {
            return Err(NdJsonSpatialError::Error(format!(
                "unknown aggregate function '{}'",
                function
            )));
        }
        names.push(format!("{}_{}", field_name(&path), function));
        let args = if function == "count" {
            vec![]
        } else {
            vec![Expr::Term(Term::Selector(path))]
        };
        items.push(Expr::Call(function.clone(), args));
    }

    let error = |e: rmath_ast::ResolveError| NdJsonSpatialError::Error(e.to_string());
    let list = AggregateList::compile(&registry, &items, &by).map_err(error)?;
    let unbounded = list.unbounded_functions();
    if !unbounded.is_empty() && !options.allow_unbounded {
        return Err(NdJsonSpatialError::Error(format!(
            "{} use{} memory that grows with the input; pass --allow-unbounded",
            unbounded.join(", "),
            if unbounded.len() == 1 { "s" } else { "" }
        )));
    }
    let aggregation = Aggregation {
        names,
        list,
        by: by
            .iter()
            .map(|b| registry.resolve(b))
            .collect::<Result<_, _>>()
            .map_err(error)?,
        grouping: options.grouping,
    };
    group(&aggregation, input, &mut output)
}

fn parse_selector(selector: &str) -> Result<Vec<Selector>, NdJsonSpatialError> {
    legacy_selector(selector)
        .ok_or_else(|| NdJsonSpatialError::Error(format!("invalid selector {:?}", selector)))
}

/// `d.address.state` → `address_state`, `d.tags[0]` → `tags_0`.
fn field_name(path: &[Selector]) -> String {
    let parts: Vec<_> = path
        .iter()
        .map(|s| match s {
            Selector::Identifier(ident) => ident.clone(),
            Selector::Index(i) => i.to_string(),
        })
        .collect();
    parts.join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn agg(
        group_by: &[&str],
        aggregates: &[(&str, &str)],
        grouping: Grouping,
        input: &str,
    ) -> Result<Vec<Value>, NdJsonSpatialError> {
        let options = AggOptions {
            group_by: group_by.iter().map(|s| s.to_string()).collect(),
            aggregates: aggregates
                .iter()
                .map(|(f, s)| (f.to_string(), s.to_string()))
                .collect(),
            grouping,
            allow_unbounded: false,
        };
        let mut output = vec![];
        aggregate(&options, &mut input.as_bytes(), &mut output)?;
        Ok(String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect())
    }

    const HASH: Grouping = Grouping::Hash {
        max_groups: 1_000_000,
    };
    const INPUT: &str = "{\"foo\": \"baz\", \"quz\": 3}\n{\"foo\": \"bar\",\"quz\": 2}\n{\"foo\":\"bar\",\"quz\": 5.5}\n";

    #[test]
    fn test_agg_one_aggregate_per_run_as_before() {
        for (f, expected) in [
            ("sum", [json!(7.5), json!(3)]),
            ("min", [json!(2), json!(3)]),
            ("max", [json!(5.5), json!(3)]),
            ("count", [json!(2), json!(1)]),
        ] {
            let name = format!("quz_{}", f);
            assert_eq!(
                agg(&["d.foo"], &[(f, "d.quz")], HASH, INPUT).unwrap(),
                vec![
                    json!({"foo": "bar", name.clone(): expected[0]}),
                    json!({"foo": "baz", name: expected[1]}),
                ]
            );
        }
    }

    #[test]
    fn test_agg_several_aggregates_and_keys() {
        let input = "{\"s\": \"MN\", \"c\": \"Hennepin\", \"pop\": 3}\n{\"s\": \"MN\", \"c\": \"Hennepin\", \"pop\": 4}\n{\"s\": \"MN\", \"c\": \"Ramsey\", \"pop\": 1}\n";
        let expected = vec![
            json!({"s": "MN", "c": "Hennepin", "pop_sum": 7, "pop_avg": 3.5}),
            json!({"s": "MN", "c": "Ramsey", "pop_sum": 1, "pop_avg": 1.0}),
        ];
        for grouping in [HASH, Grouping::Sorted { check_order: true }] {
            assert_eq!(
                agg(
                    &["d.s", "d.c"],
                    &[("sum", "d.pop"), ("avg", "d.pop")],
                    grouping,
                    input
                )
                .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn test_agg_nested_and_quoted_selectors() {
        let input = "{\"foo\": {\"bar\": \"quz\",\"baz\": 4}}\n";
        assert_eq!(
            agg(&["d.\"foo\".\"bar\""], &[("sum", "d.foo.baz")], HASH, input).unwrap(),
            vec![json!({"foo_bar": "quz", "foo_baz_sum": 4})]
        );
    }

    #[test]
    fn test_agg_errors() {
        assert_eq!(
            agg(&["d.foo"], &[("median", "d.quz")], HASH, INPUT).unwrap_err(),
            NdJsonSpatialError::Error("unknown aggregate function 'median'".into())
        );
        assert_eq!(
            agg(&["d.foo"], &[("abs", "d.quz")], HASH, INPUT).unwrap_err(),
            NdJsonSpatialError::Error("unknown aggregate function 'abs'".into())
        );
        assert_eq!(
            agg(&["d.foo"], &[("collect", "d.quz")], HASH, INPUT).unwrap_err(),
            NdJsonSpatialError::Error(
                "collect uses memory that grows with the input; pass --allow-unbounded".into()
            )
        );
        assert!(matches!(
            agg(&["d.foo"], &[("sum", "d.quz")], Grouping::Sorted { check_order: true }, INPUT),
            Err(NdJsonSpatialError::Error(e)) if e.contains("must be sorted")
        ));
    }
}
