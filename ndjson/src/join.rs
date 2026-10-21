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
use serde_json::{Number, Value};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    io::{BufRead, Write},
};

pub fn join<B: BufRead, S: BufRead, O: Write>(
    mut reference_reader: &mut B,
    reference_fields: Vec<Vec<Selector>>,
    stream_fields: Vec<Vec<Selector>>,
    stream: S,
    mut out: O,
) -> Result<(), NdJsonSpatialError> {
    let references: Vec<BTreeMap<OrderedValue, Vec<Value>>> = reference_fields
        .into_iter()
        .map(|reference_field| {
            let mut references = BTreeMap::new();

            for g in NdjsonReader::new(&mut reference_reader).flatten() {
                if g.is_object() {
                    match select_from_json_object(g.clone(), &reference_field) {
                        Ok(field_value) => {
                            let field_value: OrderedValue = field_value.into();

                            references
                                .entry(field_value)
                                .and_modify(|v: &mut Vec<Value>| v.push(g.clone()))
                                .or_insert_with(|| vec![g]);
                        }
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
            references
        })
        .collect();

    for val in NdjsonReader::new(stream) {
        stream_fields
            .iter()
            .map(|identifiers| match val.clone() {
                Ok(value) => select_from_json_object(value, identifiers).ok(),
                Err(e) => {
                    writeln!(std::io::stderr(), "Error reading: {:?}", e)
                        .expect("Unable to write to stderr");
                    None
                }
            })
            .zip(references.iter())
            .for_each(|(v, references)| {
                let v: Option<OrderedValue> = v.map(|v| v.into());

                if let Some(v) = v {
                    if !matches!(
                        v,
                        OrderedValue::Array(_) | OrderedValue::Object(_) | OrderedValue::Null
                    ) {
                        if let Some(g) = references.get(&v) {
                            for value in g {
                                if let (Value::Object(s), Ok(Value::Object(mut o))) =
                                    (value, val.clone())
                                {
                                    for (k, v) in s.into_iter() {
                                        o.insert(k.to_owned(), v.to_owned());
                                    }

                                    let value = Value::from(o);

                                    writeln!(out, "{}", value).expect("Unable to write to stdout");
                                }
                            }
                        }
                    }
                }
            })
    }
    Ok(())
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
            "{\"bar\":2,\"baz\":3,\"foo\":1}\n".as_bytes().to_vec()
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
            "{\"bar\":2,\"baz\":3,\"foo\":1}\n{\"bar\":2,\"baz\":2,\"foo\":1}\n"
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
            "{\"bar\":2,\"baz\":3,\"bub\":2,\"foo\":1}\n"
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
            "{\"brand\":[1,2,3,4],\"color\":null,\"country\":\"USA\",\"county\":\"Alameda\",\"state\":\"California\"}\n".as_bytes().to_vec()
        );
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
