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
use serde_json::ser::to_string;
use std::io::{BufRead, Write};

pub fn pick_field<I: BufRead, O: Write>(
    identifiers: Vec<Selector>,
    input: &mut I,
    mut output: O,
) -> Result<(), NdJsonSpatialError> {
    for (i, value) in NdjsonReader::new(input).enumerate() {
        let v = value?;
        if let Ok(value) = select_from_json_object(v, &identifiers) {
            match to_string(&value) {
                Ok(s) => {
                    writeln!(&mut output, "{}", s).expect("Unable to write to stdout");
                }
                Err(e) => {
                    writeln!(std::io::stderr(), "Error Serializing (input {}): {}", i, e)
                        .expect("Unable to write to stderr");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_field() {
        let mut input = "{\"foo\":1}\n{\"foo\":2}\n{\"foo\":5}\n".as_bytes();

        let mut output = vec![];

        pick_field(
            vec![Selector::Identifier("foo".to_owned())],
            &mut input,
            &mut output,
        )
        .unwrap();

        assert_eq!(output, "1\n2\n5\n".as_bytes().to_owned());
    }
}
