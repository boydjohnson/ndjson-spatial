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

use nom::{
    complete, digit, do_parse, many0, map_res, named, opt, rest, tag, take_till1, take_while,
    types::CompleteStr, whitespace::sp,
};
use serde_json::Value;
use std::{
    cmp::{Ordering, PartialOrd},
    num::{ParseFloatError, ParseIntError},
    str::{FromStr, ParseBoolError},
};
pub use yajlish::ndjson_handler::Selector;

pub type Null = Option<NonNullJsonValue>;

#[derive(Debug, PartialEq)]
pub enum NonNullJsonValue {
    String(String),
    Bool(bool),
    I64(i64),
    Float(f64),
}

impl PartialOrd for NonNullJsonValue {
    fn partial_cmp(&self, other: &NonNullJsonValue) -> Option<Ordering> {
        use NonNullJsonValue::{Bool, Float, String, I64};
        match (self, other) {
            (String(s), String(o)) => s.partial_cmp(o),
            (Bool(b), Bool(o)) => b.partial_cmp(o),
            (I64(i), I64(o)) => i.partial_cmp(o),
            (Float(f), Float(o)) => f.partial_cmp(o),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ParseNullError;

#[derive(Debug, PartialEq)]
pub enum Comparator {
    LT,
    LE,
    GT,
    GE,
    EQ,
    NE,
}

#[derive(Debug, PartialEq)]
pub struct Compare<T> {
    comparator: Comparator,
    value: T,
}

impl<T> Compare<T>
where
    T: ParseValue,
{
    pub fn compare(&self, other: Value) -> bool {
        match self.comparator {
            Comparator::LT => T::parse_value(other)
                .map(|o| o < self.value)
                .unwrap_or(false),
            Comparator::LE => T::parse_value(other)
                .map(|o| o <= self.value)
                .unwrap_or(false),
            Comparator::GT => T::parse_value(other)
                .map(|o| o > self.value)
                .unwrap_or(false),
            Comparator::GE => T::parse_value(other)
                .map(|o| o >= self.value)
                .unwrap_or(false),
            Comparator::EQ => T::parse_value(other)
                .map(|o| o == self.value)
                .unwrap_or(false),
            Comparator::NE => T::parse_value(other)
                .map(|o| o != self.value)
                .unwrap_or(false),
        }
    }
}

pub trait ParseValue: Sized + PartialOrd {
    fn parse_value(val: Value) -> Option<Self>;
}

impl ParseValue for String {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(v) => Some(v),
            Value::Null => None,
            Value::Number(num) => Some(num.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

impl ParseValue for u64 {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(v) => u64::from_str(&v).ok(),
            Value::Number(num) => num.as_u64(),
            Value::Null => None,
            Value::Bool(_) => None,
            _ => None,
        }
    }
}

impl ParseValue for i64 {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(_) => None,
            Value::Number(num) => num.as_i64(),
            Value::Null => None,
            Value::Bool(_) => None,
            _ => None,
        }
    }
}

impl ParseValue for f64 {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(_) => None,
            Value::Number(num) => num.as_f64(),
            Value::Null => None,
            Value::Bool(_) => None,
            _ => None,
        }
    }
}

impl ParseValue for bool {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(_) => None,
            Value::Number(_) => None,
            Value::Null => None,
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
}

impl ParseValue for Null {
    fn parse_value(val: Value) -> Option<Self> {
        match val {
            Value::String(s) => Some(Some(NonNullJsonValue::String(s))),
            Value::Number(s) => {
                if let Some(n) = s.as_i64() {
                    Some(Some(NonNullJsonValue::I64(n)))
                } else if let Some(f) = s.as_f64() {
                    Some(Some(NonNullJsonValue::Float(f)))
                } else {
                    Some(None)
                }
            }
            Value::Null => Some(None),
            Value::Bool(b) => Some(Some(NonNullJsonValue::Bool(b))),
            _ => None,
        }
    }
}

fn parse_u64(s: CompleteStr) -> Result<u64, ParseIntError> {
    s.parse::<u64>()
}

fn parse_i64(s: CompleteStr) -> Result<i64, ParseIntError> {
    s.parse()
}

fn parse_null(s: CompleteStr) -> Result<Null, ParseNullError> {
    if let Ok(v) = s.to_string().parse::<Value>() {
        if v == Value::Null {
            return Ok(None);
        }
    }
    Err(ParseNullError)
}

fn parse_bool(s: CompleteStr) -> Result<bool, ParseBoolError> {
    s.to_string().parse()
}

fn parse_usize(s: CompleteStr) -> Result<usize, ParseIntError> {
    s.parse()
}

fn parse_f64(s: CompleteStr) -> Result<f64, ParseFloatError> {
    s.parse::<f64>()
}

fn parse_string(s: CompleteStr) -> Result<String, std::convert::Infallible> {
    s.parse()
}

named!(
    parse_self_signifier<CompleteStr, Option<Selector>>,
    do_parse!(
        tag!("d") >>
        index: opt!(complete!(parse_index)) >>
        (index.map(Selector::Index))
    )
);

named!(
    parse_index<CompleteStr, usize>,
    do_parse!(
        tag!("[") >>
        index: map_res!(digit, parse_usize) >>
        tag!("]") >>
        (index)
    )
);

named!(
    parse_dot_plus_identifier<CompleteStr, (Selector, Option<Selector>)>,
    do_parse!(
        tag!(".") >>
        identifier: take_while!(is_not_dot_or_array_bracket_or_comparator) >>
        index: opt!(parse_index) >>
        (Selector::Identifier(format!("\"{}\"", identifier)), index.map(Selector::Index))
    )
);

fn is_not_dot_or_array_bracket_or_comparator(c: char) -> bool {
    !is_dot(c) && !is_array_bracket(c) && !is_comparator(c) && c != ' '
}

fn is_dot(c: char) -> bool {
    c == '.'
}

fn is_array_bracket(c: char) -> bool {
    c == '['
}

fn combine_identifiers(
    first: Option<Selector>,
    next: Vec<(Selector, Option<Selector>)>,
) -> Vec<Selector> {
    let mut items = vec![];
    if let Some(f) = first {
        items.push(f);
    }

    for (ident, optional_second) in next {
        items.push(ident);
        if let Some(s) = optional_second {
            items.push(s);
        }
    }

    items
}

named!(
    parse_many_identifiers<CompleteStr, Vec<(Selector, Option<Selector>)>>,
    many0!(complete!(parse_dot_plus_identifier))
);

named!(
    pub parse_json_selector<CompleteStr, Vec<Selector>>,
    do_parse!(
        first_array_selection: parse_self_signifier >>
        identifiers: parse_many_identifiers >>
        (combine_identifiers(first_array_selection, identifiers))
    )
);

fn is_comparator(c: char) -> bool {
    c == '<' || c == '=' || c == '!' || c == '>'
}

fn comparator(c: CompleteStr) -> Result<Comparator, ()> {
    match c.0 {
        "<" => Ok(Comparator::LT),
        "<=" => Ok(Comparator::LE),
        "==" => Ok(Comparator::EQ),
        "!=" => Ok(Comparator::NE),
        ">" => Ok(Comparator::GT),
        ">=" => Ok(Comparator::GE),
        _ => Err(()),
    }
}

named!(
    parse_comparator<CompleteStr, Comparator>,
    map_res!(take_till1!(is_digit_or_space), comparator)
);

fn is_digit(c: char) -> bool {
    c.is_digit(10)
}

fn is_digit_or_space(c: char) -> bool {
    is_digit(c) || c == ' '
}

named!(
    parse_value_f64<CompleteStr, f64>,
    map_res!(rest, parse_f64)
);

named!(
    parse_value_u64<CompleteStr, u64>,
    map_res!(rest, parse_u64)
);

named!(
    parse_value_string<CompleteStr, String>,
    map_res!(rest, parse_string)
);

named!(
    parse_compare_null<CompleteStr, Compare<Null>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_null) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_bool<CompleteStr, Compare<bool>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_bool) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_i64<CompleteStr, Compare<i64>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_i64) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_u64<CompleteStr, Compare<u64>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_u64) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_f64<CompleteStr, Compare<f64>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_f64) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_string<CompleteStr, Compare<String>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest, parse_string) >>
        (Compare { comparator, value })
    )
);

named!(
    pub parse_selector_i64<CompleteStr, (Compare<i64>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_i64 >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_null<CompleteStr, (Compare<Null>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_null >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_bool<CompleteStr, (Compare<bool>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_bool >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_u64<CompleteStr, (Compare<u64>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_u64 >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_f64<CompleteStr, (Compare<f64>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_f64 >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_string<CompleteStr, (Compare<String>, Vec<Selector>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_string >>
        (compare, identifiers)
    )
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_self_signifier_success() {
        assert_eq!(parse_self_signifier("d".into()), Ok(("".into(), None)));
        assert_eq!(
            parse_self_signifier("d[0]".into()),
            Ok(("".into(), Some(Selector::Index(0))))
        );
        assert_eq!(
            parse_self_signifier("d[24]".into()),
            Ok(("".into(), Some(Selector::Index(24))))
        );

        assert_eq!(
            parse_self_signifier("d.properties.AREA".into()),
            Ok((".properties.AREA".into(), None))
        );
    }

    #[test]
    fn test_parse_self_signifier_failure() {
        assert!(parse_self_signifier("b".into()).is_err());

        assert!(parse_self_signifier("e[]".into()).is_err());
    }

    #[test]
    fn test_dot_plus_identifier_success() {
        assert_eq!(
            parse_dot_plus_identifier(".properties.AREA".into()),
            Ok((
                ".AREA".into(),
                (Selector::Identifier("\"properties\"".to_string()), None)
            ))
        );

        assert_eq!(
            parse_dot_plus_identifier(".properties.contains[5]".into()),
            Ok((
                ".contains[5]".into(),
                (Selector::Identifier("\"properties\"".to_string()), None)
            ))
        );

        assert_eq!(
            parse_dot_plus_identifier(".contains[5]".into()),
            Ok((
                "".into(),
                (
                    Selector::Identifier("\"contains\"".to_string()),
                    Some(Selector::Index(5))
                )
            ))
        );
    }

    #[test]
    fn test_dot_plus_identifier_failure() {
        assert!(parse_dot_plus_identifier("simply.considered".into()).is_err())
    }

    #[test]
    fn test_many_identifiers() {
        assert_eq!(
            parse_many_identifiers(".properties.AREA>".into()),
            Ok((
                ">".into(),
                vec![
                    (Selector::Identifier("\"properties\"".to_string()), None),
                    (Selector::Identifier("\"AREA\"".to_string()), None)
                ]
            ))
        );
    }

    #[test]
    fn test_json_selector_success() {
        assert_eq!(
            parse_json_selector("d.properties.AREA".into()),
            Ok((
                "".into(),
                vec![
                    Selector::Identifier("\"properties\"".to_string()),
                    Selector::Identifier("\"AREA\"".to_string())
                ]
            ))
        )
    }

    #[test]
    fn test_parse_comparator_success() {
        assert_eq!(
            parse_comparator(">=5.5".into()),
            Ok(("5.5".into(), Comparator::GE))
        );

        assert_eq!(
            parse_comparator("== 7.4".into()),
            Ok((" 7.4".into(), Comparator::EQ))
        );
    }

    #[test]
    fn test_parse_value_success() {
        assert_eq!(parse_value_f64("5.5".into()), Ok(("".into(), 5.5)));

        assert_eq!(parse_value_u64("6555".into()), Ok(("".into(), 6555)));
    }

    #[test]
    fn test_parse_compare_success() {
        assert_eq!(
            parse_compare_f64(">= 5.5".into()),
            Ok((
                "".into(),
                Compare {
                    comparator: Comparator::GE,
                    value: 5.5
                }
            ))
        );

        assert_eq!(
            parse_compare_u64("== 5".into()),
            Ok((
                "".into(),
                Compare {
                    comparator: Comparator::EQ,
                    value: 5
                }
            ))
        );

        assert_eq!(
            parse_compare_f64("<= 7.4".into()),
            Ok((
                "".into(),
                Compare {
                    comparator: Comparator::LE,
                    value: 7.4
                }
            ))
        );

        assert_eq!(
            parse_compare_u64("==568473".into()),
            Ok((
                "".into(),
                Compare {
                    comparator: Comparator::EQ,
                    value: 568473
                }
            ))
        );
    }

    #[test]
    fn test_full_selector_success() {
        assert_eq!(
            parse_selector_f64("d.properties.AREA >= 5.5".into()),
            Ok((
                "".into(),
                (
                    Compare {
                        comparator: Comparator::GE,
                        value: 5.5
                    },
                    vec![
                        Selector::Identifier("\"properties\"".to_string()),
                        Selector::Identifier("\"AREA\"".to_string())
                    ]
                )
            ))
        );

        assert_eq!(
            parse_selector_u64("d[5].manager.pay >= 40000".into()),
            Ok((
                "".into(),
                (
                    Compare {
                        comparator: Comparator::GE,
                        value: 40000
                    },
                    vec![
                        Selector::Index(5),
                        Selector::Identifier("\"manager\"".to_string()),
                        Selector::Identifier("\"pay\"".to_string())
                    ]
                )
            ))
        );
    }

    #[test]
    fn test_parse_full_selector_failure() {
        assert!(parse_selector_u64("d[5].manager_pay >= 60.456".into()).is_err());

        assert!(parse_selector_f64("d[55]. manager. pay".into()).is_err());
    }
}
