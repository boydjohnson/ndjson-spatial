/*
 * Copyright 2019 Gobsmacked Labs, LLC
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

use nom::digit;
use nom::{
    complete, do_parse, many1, map_res, named, opt, rest_s, tag, take_till1, whitespace::sp,
};
use std::num::{ParseFloatError, ParseIntError};
use std::str::FromStr;

#[derive(Debug, PartialEq, Clone)]
pub struct ArraySelection {
    index: u64,
}

impl ArraySelection {
    pub fn index(&self) -> usize {
        self.index as usize
    }
}

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
    T: FromStr + ::std::cmp::PartialOrd,
{
    pub fn compare(&self, other: &str) -> bool {
        match self.comparator {
            Comparator::LT => T::from_str(other).map(|o| o < self.value).unwrap_or(false),
            Comparator::LE => T::from_str(other).map(|o| o <= self.value).unwrap_or(false),
            Comparator::GT => T::from_str(other).map(|o| o > self.value).unwrap_or(false),
            Comparator::GE => T::from_str(other).map(|o| o >= self.value).unwrap_or(false),
            Comparator::EQ => T::from_str(other).map(|o| o == self.value).unwrap_or(false),
            Comparator::NE => T::from_str(other).map(|o| o != self.value).unwrap_or(false),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum Identifier {
    Identifier(String),
    ArraySelection(ArraySelection),
}

fn parse_u64(s: &str) -> Result<u64, ParseIntError> {
    s.parse::<u64>()
}

fn parse_f64(s: &str) -> Result<f64, ParseFloatError> {
    s.parse::<f64>()
}

fn parse_string(s: &str) -> Result<String, std::convert::Infallible> {
    s.parse()
}

named!(
    parse_self_signifier<&str, Option<ArraySelection>>,
    do_parse!(
        tag!("d") >> 
        index: opt!(complete!(parse_index)) >>
        (index.map(|i| ArraySelection { index: i}))
    )
);

named!(
    parse_index<&str, u64>,
    do_parse!(
        tag!("[") >>
        index: map_res!(digit, parse_u64) >>
        tag!("]") >>
        (index)
    )
);

named!(
    parse_dot_plus_identifier<&str, (Identifier, Option<ArraySelection>)>,
    do_parse!(
        tag!(".") >>
        identifier: take_till1!(is_dot_or_array_bracket_or_comparator) >>
        index: opt!(parse_index) >>
        (Identifier::Identifier(identifier.to_string()), index.map(|i| ArraySelection {index: i}))
    )
);

fn is_dot_or_array_bracket_or_comparator(c: char) -> bool {
    is_dot(c) || is_array_bracket(c) || is_comparator(c) || c == ' '
}

fn is_dot(c: char) -> bool {
    c == '.'
}

fn is_array_bracket(c: char) -> bool {
    c == '['
}

fn combine_identifiers(
    first: Option<ArraySelection>,
    next: Vec<(Identifier, Option<ArraySelection>)>,
) -> Vec<Identifier> {
    let mut items = vec![];
    if let Some(f) = first {
        let f = Identifier::ArraySelection(f);
        items.push(f);
    }

    for (ident, optional_second) in next {
        items.push(ident);
        if let Some(s) = optional_second {
            let s = Identifier::ArraySelection(s);
            items.push(s);
        }
    }

    items
}

named!(
    parse_many_identifiers<&str, Vec<(Identifier, Option<ArraySelection>)>>,
    many1!(complete!(parse_dot_plus_identifier))
);

named!(
    pub parse_json_selector<&str, Vec<Identifier>>,
    do_parse!(
        first_array_selection: parse_self_signifier >>
        identifiers: many1!(parse_dot_plus_identifier) >>
        (combine_identifiers(first_array_selection, identifiers))
    )
);

fn is_comparator(c: char) -> bool {
    c == '<' || c == '=' || c == '!' || c == '>'
}

fn comparator(c: &str) -> Result<Comparator, ()> {
    match c {
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
    parse_comparator<&str, Comparator>,
    map_res!(take_till1!(is_digit_or_space), comparator)
);

fn is_digit(c: char) -> bool {
    c.is_digit(10)
}

fn is_digit_or_space(c: char) -> bool {
    is_digit(c) || c == ' '
}

named!(
    parse_value_f64<&str, f64>,
    map_res!(rest_s, parse_f64)
);

named!(
    parse_value_u64<&str, u64>,
    map_res!(rest_s, parse_u64)
);

named!(
    parse_value_string<&str, String>,
    map_res!(rest_s, parse_string)
);

named!(
    parse_compare_u64<&str, Compare<u64>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest_s, parse_u64) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_f64<&str, Compare<f64>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest_s, parse_f64) >>
        (Compare { comparator, value })
    )
);

named!(
    parse_compare_string<&str, Compare<String>>,
    do_parse!(
        comparator: parse_comparator >>
        opt!(sp) >>
        value: map_res!(rest_s, parse_string) >>
        (Compare { comparator, value })
    )
);

named!(
    pub parse_selector_u64<&str, (Compare<u64>, Vec<Identifier>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_u64 >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_f64<&str, (Compare<f64>, Vec<Identifier>)>,
    do_parse!(
        identifiers: parse_json_selector >>
        opt!(sp) >>
        compare: parse_compare_f64 >>
        (compare, identifiers)
    )
);

named!(
    pub parse_selector_string<&str, (Compare<String>, Vec<Identifier>)>,
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
        assert_eq!(parse_self_signifier("d"), Ok(("", None)));
        assert_eq!(
            parse_self_signifier("d[0]"),
            Ok(("", Some(ArraySelection { index: 0 })))
        );
        assert_eq!(
            parse_self_signifier("d[24]"),
            Ok(("", Some(ArraySelection { index: 24 })))
        );

        assert_eq!(
            parse_self_signifier("d.properties.AREA"),
            Ok((".properties.AREA", None))
        );
    }

    #[test]
    fn test_parse_self_signifier_failure() {
        assert!(parse_self_signifier("b").is_err());

        assert!(parse_self_signifier("e[]").is_err());
    }

    #[test]
    fn test_dot_plus_identifier_success() {
        assert_eq!(
            parse_dot_plus_identifier(".properties.AREA"),
            Ok((
                ".AREA",
                (Identifier::Identifier("properties".to_string()), None)
            ))
        );

        assert_eq!(
            parse_dot_plus_identifier(".properties.contains[5]"),
            Ok((
                ".contains[5]",
                (Identifier::Identifier("properties".to_string()), None)
            ))
        );

        assert_eq!(
            parse_dot_plus_identifier(".contains[5]"),
            Ok((
                "",
                (
                    Identifier::Identifier("contains".to_string()),
                    Some(ArraySelection { index: 5 })
                )
            ))
        );
    }

    #[test]
    fn test_dot_plus_identifier_failure() {
        assert!(parse_dot_plus_identifier("simply.considered").is_err())
    }

    #[test]
    fn test_many_identifiers() {
        assert_eq!(
            parse_many_identifiers(".properties.AREA>"),
            Ok((
                ">",
                vec![
                    (Identifier::Identifier("properties".to_string()), None),
                    (Identifier::Identifier("AREA".to_string()), None)
                ]
            ))
        );
    }

    #[test]
    fn test_json_selector_success() {
        assert_eq!(
            parse_json_selector("d.properties.AREA "),
            Ok((
                " ",
                vec![
                    Identifier::Identifier("properties".to_string()),
                    Identifier::Identifier("AREA".to_string())
                ]
            ))
        )
    }

    #[test]
    fn test_parse_comparator_success() {
        assert_eq!(parse_comparator(">=5.5"), Ok(("5.5", Comparator::GE)));

        assert_eq!(parse_comparator("== 7.4"), Ok((" 7.4", Comparator::EQ)));
    }

    #[test]
    fn test_parse_value_success() {
        assert_eq!(parse_value_f64("5.5"), Ok(("", 5.5)));

        assert_eq!(parse_value_u64("6555"), Ok(("", 6555)));
    }

    #[test]
    fn test_parse_compare_success() {
        assert_eq!(
            parse_compare_f64(">= 5.5"),
            Ok((
                "",
                Compare {
                    comparator: Comparator::GE,
                    value: 5.5
                }
            ))
        );

        assert_eq!(
            parse_compare_u64("== 5"),
            Ok((
                "",
                Compare {
                    comparator: Comparator::EQ,
                    value: 5
                }
            ))
        );

        assert_eq!(
            parse_compare_f64("<= 7.4"),
            Ok((
                "",
                Compare {
                    comparator: Comparator::LE,
                    value: 7.4
                }
            ))
        );

        assert_eq!(
            parse_compare_u64("==568473"),
            Ok((
                "",
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
            parse_selector_f64("d.properties.AREA >= 5.5"),
            Ok((
                "",
                (
                    Compare {
                        comparator: Comparator::GE,
                        value: 5.5
                    },
                    vec![
                        Identifier::Identifier("properties".to_string()),
                        Identifier::Identifier("AREA".to_string())
                    ]
                )
            ))
        );

        assert_eq!(
            parse_selector_u64("d[5].manager.pay >= 40000"),
            Ok((
                "",
                (
                    Compare {
                        comparator: Comparator::GE,
                        value: 40000
                    },
                    vec![
                        Identifier::ArraySelection(ArraySelection { index: 5 }),
                        Identifier::Identifier("manager".to_string()),
                        Identifier::Identifier("pay".to_string())
                    ]
                )
            ))
        );
    }

    #[test]
    fn test_parse_full_selector_failure() {
        assert!(parse_selector_u64("d[5].manager_pay >= 60.456").is_err());

        assert!(parse_selector_f64("d[55]. manager. pay").is_err());
    }
}
