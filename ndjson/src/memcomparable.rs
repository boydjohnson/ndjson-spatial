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

//! An order-preserving byte encoding of JSON values.
//!
//! Comparing two encodings with `memcmp` gives the same answer as comparing
//! the values as [`OrderedValue`](crate::join::OrderedValue)s:
//! `null < false < true < numbers < strings < arrays < objects`, numbers
//! compared numerically across integers and floats, strings byte-wise,
//! arrays lexicographically, and objects by their sorted `(key, value)`
//! pairs.
//!
//! Every encoding is prefix-free (no encoding is a prefix of another), so a
//! tuple of values is encoded by concatenating them, and a descending
//! component is encoded by inverting its bytes.

use serde_json::{Number, Value};

const END: u8 = 0x00;
const KEY: u8 = 0x01;
const NULL: u8 = 0x01;
const FALSE: u8 = 0x02;
const TRUE: u8 = 0x03;
const NUMBER: u8 = 0x04;
const STRING: u8 = 0x05;
const ARRAY: u8 = 0x06;
const OBJECT: u8 = 0x07;

/// Appends the encoding of `value` to `out`, inverted when `descending`.
pub fn encode(value: &Value, descending: bool, out: &mut Vec<u8>) {
    let start = out.len();
    encode_value(value, out);
    if descending {
        for b in &mut out[start..] {
            *b = !*b;
        }
    }
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(NULL),
        Value::Bool(false) => out.push(FALSE),
        Value::Bool(true) => out.push(TRUE),
        Value::Number(n) => {
            out.push(NUMBER);
            encode_number(n, out);
        }
        Value::String(s) => {
            out.push(STRING);
            encode_bytes(s.as_bytes(), out);
        }
        Value::Array(items) => {
            out.push(ARRAY);
            for item in items {
                encode_value(item, out);
            }
            out.push(END);
        }
        Value::Object(map) => {
            out.push(OBJECT);
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            for (k, v) in entries {
                out.push(KEY);
                encode_bytes(k.as_bytes(), out);
                encode_value(v, out);
            }
            out.push(END);
        }
    }
}

/// A number is its nearest `f64` followed by the exact distance from that
/// `f64`, which is non-zero only for integers beyond 2^53. Rounding to the
/// nearest `f64` never reorders numbers, and within one `f64` the distance
/// orders the integers that round to it.
fn encode_number(n: &Number, out: &mut Vec<u8>) {
    let (approx, distance) = if let Some(u) = n.as_u64() {
        let f = u as f64;
        (f, i128::from(u) - f as i128)
    } else if let Some(i) = n.as_i64() {
        let f = i as f64;
        (f, i128::from(i) - f as i128)
    } else {
        (n.as_f64().unwrap_or(f64::NAN), 0)
    };
    let approx = if approx == 0.0 {
        0.0 // -0.0 == 0.0
    } else if approx.is_nan() {
        f64::NAN // one NaN, after every other number
    } else {
        approx
    };

    let bits = approx.to_bits();
    let bits = if bits >> 63 == 1 {
        !bits
    } else {
        bits | 1 << 63
    };
    out.extend_from_slice(&bits.to_be_bytes());
    // |distance| < 2^11 for 64-bit integers
    let distance = distance as i64 as u64 ^ 1 << 63;
    out.extend_from_slice(&distance.to_be_bytes());
}

/// `0x00` is escaped as `0x00 0xff` and the string ends with `0x00 0x01`,
/// so a string sorts before any longer string it is a prefix of.
fn encode_bytes(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xff);
        }
    }
    out.extend_from_slice(&[0x00, 0x01]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::OrderedValue;
    use serde_json::json;

    fn values() -> Vec<Value> {
        vec![
            json!(null),
            json!(false),
            json!(true),
            json!(i64::MIN),
            json!(-9007199254740993i64),
            json!(-9007199254740992.0),
            json!(-2.5),
            json!(-1),
            json!(-0.0),
            json!(0),
            json!(0.5),
            json!(1),
            json!(1.0),
            json!(2.5),
            json!(3),
            json!(9007199254740992u64),
            json!(9007199254740993u64),
            json!(9007199254740994.0),
            json!(u64::MAX),
            json!(18446744073709551616.0),
            json!(f64::MAX),
            json!(""),
            json!("\u{0}"),
            json!("\u{0}a"),
            json!("a"),
            json!("a\u{0}"),
            json!("ab"),
            json!("b"),
            json!("é"),
            json!([]),
            json!([null]),
            json!([1]),
            json!([1, 2]),
            json!([1, "a"]),
            json!([2]),
            json!([[]]),
            json!({}),
            json!({"": 1}),
            json!({"\u{0}": 1}),
            json!({"a": 1}),
            json!({"a": 1, "b": 2}),
            json!({"a": 2}),
            json!({"a": "x"}),
            json!({"b": null}),
        ]
    }

    fn encoded(v: &Value, descending: bool) -> Vec<u8> {
        let mut out = vec![];
        encode(v, descending, &mut out);
        out
    }

    #[test]
    fn test_encoding_matches_ordered_value() {
        let values = values();
        for a in &values {
            for b in &values {
                let expected = OrderedValue::from(a.clone()).cmp(&OrderedValue::from(b.clone()));
                assert_eq!(
                    encoded(a, false).cmp(&encoded(b, false)),
                    expected,
                    "{} vs {}",
                    a,
                    b
                );
                assert_eq!(
                    encoded(a, true).cmp(&encoded(b, true)),
                    expected.reverse(),
                    "{} vs {} descending",
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn test_tuples_compare_component_wise() {
        let values = values();
        let tuple = |a: &Value, b: &Value| {
            let mut out = vec![];
            encode(a, false, &mut out);
            encode(b, true, &mut out);
            out
        };
        for a in &values {
            for b in &values {
                for c in &values {
                    for d in &values[..8] {
                        let expected = OrderedValue::from(a.clone())
                            .cmp(&OrderedValue::from(c.clone()))
                            .then_with(|| {
                                OrderedValue::from(d.clone()).cmp(&OrderedValue::from(b.clone()))
                            });
                        assert_eq!(tuple(a, b).cmp(&tuple(c, d)), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn test_equal_numbers_encode_equally() {
        assert_eq!(encoded(&json!(1), false), encoded(&json!(1.0), false));
        assert_eq!(encoded(&json!(0), false), encoded(&json!(-0.0), false));
    }

    #[test]
    fn test_object_key_order_does_not_matter() {
        // serde_json keeps input key order (preserve_order)
        let ab: Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        let ba: Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        assert_eq!(encoded(&ab, false), encoded(&ba, false));
    }
}
