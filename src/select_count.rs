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
use crate::filter::select_from_json_object;
use crate::json_parser::{
    parse_json_selector, parse_selector_f64, parse_selector_u64, Compare, Identifier,
};
use crate::ndjson::NdJsonGeojsonReader;
use geojson::GeoJson;
use num_traits::Num;
use serde_json::Value;
use std::io::Write;

pub fn select_count(
    expression: &str,
    selector: &str,
    field_name: &str,
) -> Result<(), NdJsonSpatialError> {
    if let Ok((_, exp_identifiers)) = parse_json_selector(expression) {
        if let Ok((_, (compare, identifiers))) = parse_selector_u64(selector) {
            count_and_then_write_to_stdout(exp_identifiers, compare, identifiers, field_name)?;
        } else if let Ok((_, (compare, identifiers))) = parse_selector_f64(selector) {
            count_and_then_write_to_stdout(exp_identifiers, compare, identifiers, field_name)?;
        }
    }
    Ok(())
}

fn count_and_then_write_to_stdout<T>(
    exp_identifiers: Vec<Identifier>,
    compare: Compare<T>,
    identifiers: Vec<Identifier>,
    field_name: &str,
) -> Result<(), NdJsonSpatialError>
where
    T: Num + PartialOrd,
{
    for value in NdJsonGeojsonReader::default() {
        let v = value?;
        if let GeoJson::Feature(mut feature) = v.clone() {
            let mut count = 0;

            if let Some(o) = &feature.properties {
                if let Ok(value) =
                    select_from_json_object(Value::Object(o.clone()), &exp_identifiers)
                {
                    if let Value::Array(a) = value {
                        for item in &a {
                            if let Ok(val) = select_from_json_object(item.clone(), &identifiers) {
                                match val {
                                    Value::Number(n) => {
                                        if compare.compare(&n.to_string()) {
                                            count += 1;
                                        }
                                    }
                                    Value::String(s) => {
                                        if compare.compare(&s) {
                                            count += 1;
                                        }
                                    }
                                    _ => (),
                                }
                            }
                        }
                    }
                }
            }

            if let Some(p) = feature.properties.as_mut() {
                p.insert(field_name.to_string(), Value::Number(count.into()));
            };
        }

        writeln!(::std::io::stdout(), "{}", v).expect("Could not write to stdout");
    }
    Ok(())
}
