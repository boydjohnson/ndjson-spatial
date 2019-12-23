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

use crate::error::NdJsonSpatialError;
use crate::json_selector_parser::Identifier;
use encode_unicode::{error::InvalidUtf8Slice, IterExt, Utf8Char};
use std::io::{BufReader, Read, Write};

const LEFT_BRACE: char = '{';
const RIGHT_BRACE: char = '}';
const LEFT_BRACKET: char = '[';
const RIGHT_BRACKET: char = ']';
const NEW_LINE_DELIMITER: char = '\n';

pub fn generic_split<R: Read, W: Write>(
    read: R,
    mut write: W,
    name: &str,
) -> Result<(), NdJsonSpatialError> {
    let mut string_until_feature = String::new();

    let r = BufReader::new(read);
    let mut bytes_iter = r.bytes().scan(0, |_, result| result.ok()).to_utf8chars();

    for byte in &mut bytes_iter {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();

        string_until_feature.push(c);
        if string_until_feature.contains(name) {
            break;
        }
    }
    let mut json_string = String::new();
    let mut start = false;
    let mut count = 0;
    for byte in &mut bytes_iter {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();
        if c == LEFT_BRACE {
            count += 1;
            start = true;
        }
        if start && c != NEW_LINE_DELIMITER {
            json_string.push(c);
        }

        if c == RIGHT_BRACE {
            count -= 1;
        }
        if count == 0 && start {
            if let Err(err) = writeln!(write, "{}", json_string) {
                writeln!(std::io::stderr(), "{}", err).expect("Unable to write to stderr");
            }
            json_string = String::new();
            start = false;
        }
    }

    Ok(())
}

struct FindIdentifierState {
    nesting_level: u32,
    nesting_level_count: u32,
    string_until_feature: String,
}

impl FindIdentifierState {
    fn new() -> FindIdentifierState {
        FindIdentifierState {
            nesting_level: 1,
            nesting_level_count: 0,
            string_until_feature: String::new(),
        }
    }

    fn inc_nesting_level_count(&mut self) {
        self.nesting_level_count += 1;
    }

    fn dec_nesting_level_count(&mut self) {
        self.nesting_level_count -= 1;
    }

    fn inc_nesting_level(&mut self) {
        self.nesting_level += 1;
    }

    fn contains_identifier(&self, other: &str) -> bool {
        self.string_until_feature.contains(other)
    }

    fn add_to_string(&mut self, c: char) {
        self.string_until_feature.push(c);
    }

    fn identifier_in_correct_nesting_level(&self) -> bool {
        self.nesting_level == self.nesting_level_count
    }
}

pub fn generic_split_identifiers<R: Read, W: Write>(
    read: R,
    mut writer: W,
    identifiers: Vec<Identifier>,
) -> Result<(), NdJsonSpatialError> {
    let mut find_state = FindIdentifierState::new();

    let r = BufReader::new(read);
    let mut char_iter: Box<dyn Iterator<Item = Result<Utf8Char, InvalidUtf8Slice>>> =
        Box::new(r.bytes().scan(0, |_, result| result.ok()).to_utf8chars());

    let mut identifiers_iter = identifiers.into_iter().peekable();

    for byte in &mut char_iter {
        let b: char = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();
        let identifier = identifiers_iter.peek();
        if let Some(Identifier::Identifier(ident)) = identifier {
            if b == LEFT_BRACE {
                find_state.inc_nesting_level_count();
            }
            if b == RIGHT_BRACE {
                find_state.dec_nesting_level_count();
            }

            find_state.add_to_string(b);
            let ident = format!(" \"{}\"", ident);
            if find_state.contains_identifier(&ident)
                && find_state.identifier_in_correct_nesting_level()
            {
                identifiers_iter.next();
                find_state.inc_nesting_level();

                if identifiers_iter.peek().is_none() {
                    break;
                }
            }
        }
    }

    let mut json_string = String::new();
    let mut start_braces = false;
    let mut start_brackets = false;
    let mut braces = find_state.nesting_level;
    let mut brackets = 0;
    let mut double_brackets = false;
    for byte in &mut char_iter {
        let byte: char = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();

        if byte == LEFT_BRACE {
            braces += 1;
            if !double_brackets {
                start_braces = true;
            }
        }

        if byte == LEFT_BRACKET {
            if brackets == 1 {
                double_brackets = true;
            }

            if !start_braces {
                start_brackets = true;
            }

            brackets += 1;
        }
        if (double_brackets && start_brackets || start_braces) && byte != NEW_LINE_DELIMITER {
            json_string.push(byte);
        }

        if byte == RIGHT_BRACKET {
            brackets -= 1;
        }

        if byte == RIGHT_BRACE {
            braces -= 1;
        }
        if (braces == find_state.nesting_level && start_braces)
            || (brackets == 1 && double_brackets && start_brackets)
        {
            writeln!(writer, "{}", json_string).expect("Error writing to stdout");
            json_string = String::new();
            start_brackets = false;
            start_braces = false;
        }

        if braces < find_state.nesting_level {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::generic_split_identifiers;
    use crate::json_selector_parser::Identifier;

    #[test]
    fn test_generic_split_identifiers_test_identifiers() {
        let identifiers = vec![Identifier::Identifier("data".to_string())];

        let in_buffer =
            "{ \"other\": [{\"junk\": \"junk\"}], \"data\": [{\"key\": \"value\"}]}".as_bytes();
        let mut out = Vec::with_capacity(50);

        generic_split_identifiers(in_buffer, &mut out, identifiers)
            .expect("Able to successfully call function");

        assert_eq!(out, "{\"key\": \"value\"}\n".as_bytes());

        let identifiers = vec![Identifier::Identifier("foo".to_string())];

        let in_buffer =
            "{ \"other\": [{\"junk\": \"junk\"}], \"foo\": [{\"key\": \"value\"}, {\"key\": \"value\"}] }".as_bytes();
        let mut out = Vec::with_capacity(100);

        generic_split_identifiers(in_buffer, &mut out, identifiers)
            .expect("Able to successfully call function");
        assert_eq!(
            out,
            "{\"key\": \"value\"}\n{\"key\": \"value\"}\n".as_bytes()
        );

        let identifiers = vec![Identifier::Identifier("welp".to_string())];

        let in_buffer = "{ \"welp\": [[\"foo\", \"bar\"],[123, 567]]}".as_bytes();

        let mut out = Vec::with_capacity(100);
        generic_split_identifiers(in_buffer, &mut out, identifiers).expect("Able to call function");
        assert_eq!(out, "[\"foo\", \"bar\"]\n[123, 567]\n".as_bytes());

        let identifiers = vec![
            Identifier::Identifier("meta".to_string()),
            Identifier::Identifier("view".to_string()),
        ];

        let in_buffer =
            "{ \"meta\": { \"view\": [{ \"key\": \"value\"}]}, \"other\": { \"other\": \"junk\"}}"
                .as_bytes();
        let mut out = Vec::with_capacity(100);

        generic_split_identifiers(in_buffer, &mut out, identifiers)
            .expect("Able to successfully call function");
        assert_eq!(out, "{ \"key\": \"value\"}\n".as_bytes());
    }
}
