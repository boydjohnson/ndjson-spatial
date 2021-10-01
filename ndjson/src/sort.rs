use crate::{filter::select_from_json_object, join::OrderedValue};
use itertools::Itertools;
use ndjson_common::{
    error::NdJsonSpatialError, json_selector_parser::Selector, ndjson::NdjsonReader,
};

use std::{
    cmp::Ordering,
    io::{BufRead, Write},
};

pub fn sort<IN: BufRead, OUT: Write>(
    input: IN,
    output: &mut OUT,
    selectors: Vec<(Vec<Selector>, bool)>,
) -> Result<(), NdJsonSpatialError> {
    for item in NdjsonReader::new(input).flatten().sorted_by(|left, right| {
        selectors
            .iter()
            .map(|(selector, ascending)| {
                if *ascending {
                    Ord::cmp(
                        &select_from_json_object(left.clone(), selector).map(OrderedValue::from),
                        &select_from_json_object(right.clone(), selector).map(OrderedValue::from),
                    )
                } else {
                    Ord::cmp(
                        &select_from_json_object(right.clone(), selector).map(OrderedValue::from),
                        &select_from_json_object(left.clone(), selector).map(OrderedValue::from),
                    )
                }
            })
            .fold(Ordering::Equal, |acc, item| acc.then(item))
    }) {
        writeln!(output, "{}", item).expect("Unable to write to stdout");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sort_already_sorted() {
        let input = "{\"bar\":3,\"foo\":4}\n{\"bar\":4,\"foo\":7}\n";

        let mut output = vec![];

        sort(
            input.as_bytes(),
            &mut output,
            vec![(vec![Selector::Identifier("bar".into())], true)],
        )
        .unwrap();

        assert_eq!(output, input.as_bytes());

        let mut output = vec![];

        sort(
            input.as_bytes(),
            &mut output,
            vec![
                (vec![Selector::Identifier("foo".into())], true),
                (vec![Selector::Identifier("bar".into())], true),
            ],
        )
        .unwrap();

        assert_eq!(output, input.as_bytes());
    }

    #[test]
    fn test_sort_descending() {
        let input = "{\"bar\":3,\"foo\":4}\n{\"bar\":4,\"foo\":7}\n";

        let mut output = vec![];

        sort(
            input.as_bytes(),
            &mut output,
            vec![(vec![Selector::Identifier("bar".into())], false)],
        )
        .unwrap();

        assert_eq!(
            output,
            "{\"bar\":4,\"foo\":7}\n{\"bar\":3,\"foo\":4}\n".as_bytes()
        );
    }
}
