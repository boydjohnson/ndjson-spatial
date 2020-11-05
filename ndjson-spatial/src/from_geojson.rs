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

use ndjson_common::{
    error::NdJsonSpatialError, from::generic_split, json_selector_parser::Selector,
};

pub fn split() -> Result<(), NdJsonSpatialError> {
    generic_split(
        &mut std::io::stdin().lock(),
        std::io::stdout().lock(),
        vec![Selector::Identifier("features".to_string())],
    )?;
    Ok(())
}
