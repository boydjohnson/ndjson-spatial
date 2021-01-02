use crate::{
    filter::select_from_json_object,
    join::{OrderedNumber, OrderedValue},
};
use itertools::Itertools;
use ndjson_common::{
    error::NdJsonSpatialError, json_selector_parser::Selector, ndjson::NdjsonReader,
};
use serde_json::{Map, Value};
use std::io::{BufRead, Write};

pub enum Aggregation {
    Count(Vec<Selector>),
    Sum(Vec<Selector>),
    Min(Vec<Selector>),
    Max(Vec<Selector>),
}

fn strip_quotes(ident: &str) -> String {
    ident
        .strip_prefix('"')
        .map(|s| s.strip_suffix('"'))
        .flatten()
        .unwrap_or(ident)
        .to_owned()
}

pub fn aggregate<I: BufRead, O: Write>(
    aggregator: Aggregation,
    group_by: Vec<Selector>,
    input: &mut I,
    mut output: O,
) -> Result<(), NdJsonSpatialError> {
    let named_group_by = group_by
        .iter()
        .map(|s| match s {
            Selector::Identifier(ident) => strip_quotes(ident),
            Selector::Index(i) => i.to_string(),
        })
        .collect::<Vec<String>>()
        .join("_");

    let iter = NdjsonReader::new(input)
        .sorted_by_key(|el| {
            let key: Result<OrderedValue, _> =
                select_from_json_object(el.clone()?, &group_by).map(|v| v.into());
            key
        })
        .group_by(|el| select_from_json_object(el.clone()?, &group_by));

    for (key, group) in &iter {
        let key = key?;

        let mut named_map = Map::new();

        named_map.insert(named_group_by.clone(), key);

        match &aggregator {
            Aggregation::Count(sel) => {
                let mut count_key = sel
                    .iter()
                    .map(|s| match s {
                        Selector::Identifier(ident) => strip_quotes(ident),
                        Selector::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<String>>()
                    .join("_");

                count_key.push_str("_count");

                let count = group.count();

                named_map.insert(count_key, Value::from(count));
            }
            Aggregation::Sum(sel) => {
                let mut sum_key = sel
                    .iter()
                    .map(|s| match s {
                        Selector::Identifier(ident) => strip_quotes(ident),
                        Selector::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<String>>()
                    .join("_");

                let number: Result<f64, _> = group
                    .map(|el| select_from_json_object(el?, &sel))
                    .filter_map_ok(|v| {
                        if let Value::Number(n) = v {
                            n.as_f64()
                        } else {
                            None
                        }
                    })
                    .sum();

                sum_key.push_str("_sum");

                named_map.insert(sum_key, Value::from(number?));
            }
            Aggregation::Min(sel) => {
                let mut min_key = sel
                    .iter()
                    .map(|s| match s {
                        Selector::Identifier(ident) => strip_quotes(ident),
                        Selector::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<String>>()
                    .join("_");

                let min: Result<Option<OrderedNumber>, NdJsonSpatialError> = group
                    .map(|el| select_from_json_object(el?, &sel))
                    .filter_map_ok(|v| {
                        if let Value::Number(n) = v {
                            let num: OrderedNumber = n.into();
                            Some(num)
                        } else {
                            None
                        }
                    })
                    .try_fold(None, |min: Option<OrderedNumber>, el| {
                        let element = el?;
                        if let Some(m) = min {
                            if m > element {
                                Ok(Some(element))
                            } else {
                                Ok(Some(m))
                            }
                        } else {
                            Ok(Some(element))
                        }
                    });

                min_key.push_str("_min");
                match min? {
                    Some(OrderedNumber::Float(num)) => {
                        named_map.insert(min_key, Value::from(num.0));
                    }
                    Some(OrderedNumber::PosInt(num)) => {
                        named_map.insert(min_key, Value::from(num));
                    }
                    Some(OrderedNumber::NegInt(num)) => {
                        named_map.insert(min_key, Value::from(num));
                    }
                    None => {
                        named_map.insert(min_key, Value::Null);
                    }
                }
            }
            Aggregation::Max(sel) => {
                let mut max_key = sel
                    .iter()
                    .map(|s| match s {
                        Selector::Identifier(ident) => strip_quotes(ident),
                        Selector::Index(i) => i.to_string(),
                    })
                    .collect::<Vec<String>>()
                    .join("_");

                let max: Result<Option<OrderedNumber>, NdJsonSpatialError> = group
                    .map(|el| select_from_json_object(el?, &sel))
                    .filter_map_ok(|v| {
                        if let Value::Number(n) = v {
                            Some(n.into())
                        } else {
                            None
                        }
                    })
                    .try_fold(None, |mut max, el| {
                        let element = el?;
                        if let Some(m) = max {
                            if m < element {
                                max = Some(element)
                            }
                        } else {
                            max = Some(element);
                        }
                        Ok(max)
                    });

                max_key.push_str("_max");
                match max? {
                    Some(OrderedNumber::Float(num)) => {
                        named_map.insert(max_key, Value::from(num.0));
                    }
                    Some(OrderedNumber::PosInt(num)) => {
                        named_map.insert(max_key, Value::from(num));
                    }
                    Some(OrderedNumber::NegInt(num)) => {
                        named_map.insert(max_key, Value::from(num));
                    }
                    None => {
                        named_map.insert(max_key, Value::Null);
                    }
                }
            }
        }
        match serde_json::to_string(&named_map) {
            Ok(json) => {
                writeln!(output, "{}", json).expect("Unable to write to stdout");
            }
            Err(e) => {
                writeln!(std::io::stderr(), "Error serializing JSON: {}", e)
                    .expect("Unable to write to stderr");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agg_sum() {
        let mut input = "{\"foo\": \"bar\",\"quz\": 2}\n{\"foo\":\"bar\",\"quz\": 5}\n{\"foo\":\"baz\", \"quz\": 3}\n".as_bytes();

        let mut output = vec![];

        aggregate(
            Aggregation::Sum(vec![Selector::Identifier("quz".to_owned())]),
            vec![Selector::Identifier("foo".to_owned())],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":\"bar\",\"quz_sum\":7.0}\n{\"foo\":\"baz\",\"quz_sum\":3.0}\n"
                .as_bytes()
                .to_owned()
        );
    }

    #[test]
    fn test_agg_min() {
        let mut input = "{\"foo\": \"bar\",\"quz\": 2}\n{\"foo\":\"bar\",\"quz\": 5}\n{\"foo\":\"baz\", \"quz\": 3}\n".as_bytes();

        let mut output = vec![];

        aggregate(
            Aggregation::Min(vec![Selector::Identifier("quz".to_owned())]),
            vec![Selector::Identifier("foo".to_owned())],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":\"bar\",\"quz_min\":2}\n{\"foo\":\"baz\",\"quz_min\":3}\n"
                .as_bytes()
                .to_owned()
        );
    }

    #[test]
    fn test_agg_max() {
        let mut input = "{\"foo\": \"bar\",\"quz\": 2}\n{\"foo\":\"bar\",\"quz\": 5}\n{\"foo\":\"baz\", \"quz\": 3}\n".as_bytes();

        let mut output = vec![];

        aggregate(
            Aggregation::Max(vec![Selector::Identifier("quz".to_owned())]),
            vec![Selector::Identifier("foo".to_owned())],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":\"bar\",\"quz_max\":5}\n{\"foo\":\"baz\",\"quz_max\":3}\n"
                .as_bytes()
                .to_owned()
        );
    }

    #[test]
    fn test_multiple_selectors() {
        let mut input = "{\"foo\": {\"bar\": \"quz\",\"baz\": 4}}\n".as_bytes();

        let mut output = vec![];

        aggregate(
            Aggregation::Sum(vec![
                Selector::Identifier("\"foo\"".to_owned()),
                Selector::Identifier("\"baz\"".to_owned()),
            ]),
            vec![
                Selector::Identifier("\"foo\"".to_owned()),
                Selector::Identifier("\"bar\"".to_owned()),
            ],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo_bar\":\"quz\",\"foo_baz_sum\":4.0}\n"
                .as_bytes()
                .to_owned()
        );
    }
}
