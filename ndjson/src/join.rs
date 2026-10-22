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

use crate::filter::select_from_json_object;
use ndjson_common::{
    error::NdJsonSpatialError, json_selector_parser::Selector, ndjson::NdjsonReader,
};
use ordered_float::OrderedFloat;
use serde_json::{Map, Number, Value};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    io::{BufRead, Write},
};

/// Joins each record of `stream` to the records of `reference_reader` whose
/// key matches.
///
/// The key is the tuple of the selected fields: a stream record matches a
/// reference record only when every `stream_fields[i]` equals the
/// corresponding `reference_fields[i]`. Records missing a key field, or whose
/// key contains a `null`, array or object, never match. On field collisions
/// the reference record's value wins.
pub fn join<B: BufRead, S: BufRead, O: Write>(
    reference_reader: &mut B,
    reference_fields: Vec<Vec<Selector>>,
    stream_fields: Vec<Vec<Selector>>,
    stream: S,
    mut out: O,
) -> Result<(), NdJsonSpatialError> {
    if reference_fields.is_empty() {
        return Err(NdJsonSpatialError::Error(
            "join needs at least one key field".to_owned(),
        ));
    }
    if reference_fields.len() != stream_fields.len() {
        return Err(NdJsonSpatialError::Error(format!(
            "join needs the same number of reference fields ({}) and stream fields ({})",
            reference_fields.len(),
            stream_fields.len()
        )));
    }

    let mut references: BTreeMap<Vec<OrderedValue>, Vec<Map<String, Value>>> = BTreeMap::new();

    for g in NdjsonReader::new(reference_reader).flatten() {
        if let Value::Object(o) = g {
            match select_key(&o, &reference_fields) {
                Ok(Some(key)) => references.entry(key).or_default().push(o),
                Ok(None) => (),
                Err(e) => {
                    writeln!(
                        std::io::stderr(),
                        "Unable to select from reference object: {:?}",
                        e
                    )
                    .expect("Unable to write to stderr");
                }
            }
        }
    }

    for val in NdjsonReader::new(stream) {
        let o = match val {
            Ok(Value::Object(o)) => o,
            Ok(_) => continue,
            Err(e) => {
                writeln!(std::io::stderr(), "Error reading: {:?}", e)
                    .expect("Unable to write to stderr");
                continue;
            }
        };

        let key = match select_key(&o, &stream_fields) {
            Ok(Some(key)) => key,
            Ok(None) | Err(_) => continue,
        };

        if let Some(matches) = references.get(&key) {
            for reference in matches {
                let mut joined = o.clone();
                for (k, v) in reference {
                    joined.insert(k.to_owned(), v.to_owned());
                }

                writeln!(out, "{}", Value::Object(joined)).expect("Unable to write to stdout");
            }
        }
    }
    Ok(())
}

/// Selects the composite join key from `object`. Returns `Ok(None)` when a key
/// component is a `null`, array or object, which never join.
fn select_key(
    object: &Map<String, Value>,
    fields: &[Vec<Selector>],
) -> Result<Option<Vec<OrderedValue>>, NdJsonSpatialError> {
    let mut key = Vec::with_capacity(fields.len());
    for field in fields {
        let v: OrderedValue = select_from_json_object(Value::Object(object.clone()), field)?.into();
        if matches!(
            v,
            OrderedValue::Array(_) | OrderedValue::Object(_) | OrderedValue::Null
        ) {
            return Ok(None);
        }
        key.push(v);
    }
    Ok(Some(key))
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

    #[test]
    fn test_join_simple() {
        let mut input = "{ \"foo\": 1, \"bar\": 2 }\n".as_bytes();

        let mut file = "{ \"baz\": 3, \"foo\": 1}\n".as_bytes();

        let mut output = vec![];

        join(
            &mut file,
            vec![vec![Selector::Identifier("foo".to_owned())]],
            vec![vec![Selector::Identifier("foo".to_owned())]],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":1,\"bar\":2,\"baz\":3}\n".as_bytes().to_vec()
        );
    }

    #[test]
    fn test_join_non_unique() {
        let mut input = "{ \"foo\": 1, \"bar\": 2}\n".as_bytes();

        let mut ref_file = "{ \"baz\": 3, \"foo\": 1}\n{\"foo\": 1, \"baz\": 2}\n".as_bytes();

        let mut output = vec![];

        join(
            &mut ref_file,
            vec![vec![Selector::Identifier("foo".to_owned())]],
            vec![vec![Selector::Identifier("foo".to_owned())]],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":1,\"bar\":2,\"baz\":3}\n{\"foo\":1,\"bar\":2,\"baz\":2}\n"
                .as_bytes()
                .to_vec()
        );
    }

    #[test]
    fn test_join_multiple_fields() {
        let mut input = "{ \"foo\": 1, \"bub\": 2, \"bar\": 2 }\n".as_bytes();

        let mut ref_file = "{ \"baz\": 3, \"bub\": 2, \"foo\": 1}\n".as_bytes();

        let mut output = vec![];

        join(
            &mut ref_file,
            vec![
                vec![Selector::Identifier("foo".to_owned())],
                vec![Selector::Identifier("bub".to_owned())],
            ],
            vec![
                vec![Selector::Identifier("foo".to_owned())],
                vec![Selector::Identifier("bub".to_owned())],
            ],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"foo\":1,\"bub\":2,\"bar\":2,\"baz\":3}\n"
                .as_bytes()
                .to_vec()
        );

        let mut input = "{\"country\": \"USA\",\"state\":\"California\",\"county\": \"Alameda\", \"color\": null}\n".as_bytes();

        let mut ref_file = "{\"county\": \"Alameda\", \"country\": \"USA\", \"state\": \"California\", \"brand\": [1,2,3,4]}\n".as_bytes();

        let mut output = vec![];

        join(
            &mut ref_file,
            vec![
                vec![Selector::Identifier("country".to_owned())],
                vec![Selector::Identifier("state".to_owned())],
                vec![Selector::Identifier("county".to_owned())],
            ],
            vec![
                vec![Selector::Identifier("country".to_owned())],
                vec![Selector::Identifier("state".to_owned())],
                vec![Selector::Identifier("county".to_owned())],
            ],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"country\":\"USA\",\"state\":\"California\",\"county\":\"Alameda\",\"color\":null,\"brand\":[1,2,3,4]}\n".as_bytes().to_vec()
        );
    }

    fn ident(name: &str) -> Vec<Selector> {
        vec![Selector::Identifier(name.to_owned())]
    }

    fn run_join(reference: &str, fields: &[&str], stream: &str) -> String {
        let fields: Vec<Vec<Selector>> = fields.iter().map(|f| ident(f)).collect();
        let mut output = vec![];
        join(
            &mut reference.as_bytes(),
            fields.clone(),
            fields,
            stream.as_bytes(),
            &mut output,
        )
        .unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn test_join_composite_key_requires_every_field_to_match() {
        let reference = concat!(
            "{\"state\": \"MN\", \"county\": \"Hennepin\", \"pop\": 1}\n",
            "{\"state\": \"MN\", \"county\": \"Ramsey\", \"pop\": 2}\n",
            "{\"state\": \"WI\", \"county\": \"Ramsey\", \"pop\": 3}\n",
        );
        let stream = concat!(
            "{\"state\": \"MN\", \"county\": \"Ramsey\"}\n",
            "{\"state\": \"WI\", \"county\": \"Hennepin\"}\n",
        );

        assert_eq!(
            run_join(reference, &["state", "county"], stream),
            "{\"state\":\"MN\",\"county\":\"Ramsey\",\"pop\":2}\n"
        );
    }

    #[test]
    fn test_join_composite_key_uses_later_fields() {
        // Previously only the first field was ever matched, because the
        // reference reader was exhausted after building the first index.
        let reference = "{\"a\": 1, \"b\": 1, \"r\": true}\n";
        let stream = "{\"a\": 1, \"b\": 2}\n";

        assert_eq!(run_join(reference, &["a", "b"], stream), "");
    }

    #[test]
    fn test_join_composite_key_emits_each_match_once() {
        let reference = "{\"a\": 1, \"b\": 2, \"r\": true}\n";
        let stream = "{\"a\": 1, \"b\": 2}\n{\"a\": 1, \"b\": 2}\n";

        assert_eq!(
            run_join(reference, &["a", "b"], stream),
            "{\"a\":1,\"b\":2,\"r\":true}\n{\"a\":1,\"b\":2,\"r\":true}\n"
        );
    }

    #[test]
    fn test_join_numbers_match_across_representations() {
        let reference = "{\"id\": 1.0, \"r\": true}\n";
        let stream = "{\"id\": 1}\n";

        assert_eq!(
            run_join(reference, &["id"], stream),
            "{\"id\":1.0,\"r\":true}\n"
        );
    }

    #[test]
    fn test_join_skips_null_and_missing_keys() {
        let reference = "{\"a\": null, \"b\": 1}\n{\"b\": 1}\n";
        let stream = "{\"a\": null, \"b\": 1}\n{\"b\": 1}\n";

        assert_eq!(run_join(reference, &["a", "b"], stream), "");
    }

    #[test]
    fn test_join_rejects_mismatched_field_counts() {
        let mut output = vec![];
        let result = join(
            &mut "".as_bytes(),
            vec![ident("a"), ident("b")],
            vec![ident("a")],
            "".as_bytes(),
            &mut output,
        );
        assert!(result.is_err());
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
