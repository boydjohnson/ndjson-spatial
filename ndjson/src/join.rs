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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrderedValue {
    String(String),
    Bool(bool),
    Number(OrderedNumber),
    Null,
    Array(Vec<OrderedValue>),
    Object(BTreeMap<String, OrderedValue>),
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
pub enum OrderedNumber {
    PosInt(u64),
    NegInt(i64),
    Float(OrderedFloat<f64>),
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
