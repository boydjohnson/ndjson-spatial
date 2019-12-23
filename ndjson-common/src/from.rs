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
use std::io::{BufReader, Read, Write};

pub fn generic_split<R: Read, W: Write>(
    read: R,
    mut write: W,
    name: &str,
) -> Result<(), NdJsonSpatialError> {
    let mut string_until_feature = String::new();

    let r = BufReader::new(read);
    let mut bytes_iter = r.bytes();

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
        if c == '{' {
            count += 1;
            start = true;
        }
        if start && c != '\n' {
            json_string.push(c);
        }

        if c == '}' {
            count -= 1;
        }
        if count == 0 && start {
            writeln!(write, "{}", json_string).expect("Error writing to stdout");
            json_string = String::new();
            start = false;
        }
    }

    Ok(())
}

pub fn generic_split_identifiers<R: Read, W: Write>(
    read: R,
    mut write: W,
    identifiers: Vec<Identifier>,
) -> Result<(), NdJsonSpatialError> {
    let left_brace: u8 = 123;
    let right_brace: u8 = 125;
    let new_line_delimiter: u8 = 10;

    let mut string_until_feature = String::new();

    let mut nesting_level = 1;
    let mut nesting_level_count = 0;

    let r = BufReader::new(read);
    let mut bytes_iter: Box<dyn Iterator<Item = Result<u8, std::io::Error>>> = Box::new(r.bytes());

    let mut identifiers_iter = identifiers.into_iter().peekable();

    for byte in &mut bytes_iter {
        let b: u8 = byte.map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?;
        let identifier = identifiers_iter.peek();
        if let Some(Identifier::Identifier(ident)) = identifier {
            if b == left_brace {
                nesting_level_count += 1;
            }
            if b == right_brace {
                nesting_level_count -= 1;
            }

            string_until_feature.push_str(
                std::str::from_utf8(&[b])
                    .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?,
            );
            let ident = format!(" \"{}\"", ident);
            if string_until_feature.contains(&ident) && nesting_level_count == nesting_level {
                identifiers_iter.next();
                nesting_level += 1;

                if identifiers_iter.peek().is_none() {
                    break;
                }
            }
        }
    }
    let mut json_string = String::new();
    let mut start = false;
    let mut braces = 0;
    for byte in &mut bytes_iter {
        let byte = byte.map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?;
        if byte == left_brace {
            braces += 1;
            start = true;
        }
        if start && byte != new_line_delimiter {
            json_string.push_str(
                std::str::from_utf8(&[byte])
                    .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?,
            )
        }

        if byte == right_brace {
            braces -= 1;
        }
        if braces == 0 && start {
            writeln!(write, "{}", json_string).expect("Error writing to stdout");
            json_string = String::new();
            start = false;
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
            "{ \"other\": [{\"junk\": \"junk\"}], \"foo\": [{\"key\": \"value\"}]".as_bytes();
        let mut out = Vec::with_capacity(50);

        generic_split_identifiers(in_buffer, &mut out, identifiers)
            .expect("Able to successfully call function");
        assert_eq!(out, "{\"key\": \"value\"}\n".as_bytes());
    }
}
