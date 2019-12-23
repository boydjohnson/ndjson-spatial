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

use ndjson_common::error::NdJsonSpatialError;
use ndjson_common::from::generic_split_identifiers;
use ndjson_common::json_selector_parser::parse_json_selector;

pub fn from_json(expression: &str) -> Result<(), NdJsonSpatialError> {
    let (_, identifiers) = parse_json_selector(expression.into())
        .map_err(|_| NdJsonSpatialError::Error("Could not parse json selector".to_string()))?;
    generic_split_identifiers(std::io::stdin(), std::io::stdout(), identifiers)
}
