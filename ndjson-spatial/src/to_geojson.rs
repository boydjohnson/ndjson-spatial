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

use geojson::{FeatureCollection, GeoJson};
use ndjson_common::{error::NdJsonSpatialError, ndjson::NdJsonGeojsonReader};
use std::io::Write;

pub fn to_geojson() -> Result<(), NdJsonSpatialError> {
    let mut features = vec![];
    for g in NdJsonGeojsonReader::default() {
        let g = g?;
        match g {
            GeoJson::Feature(f) => {
                features.push(f);
            }
            _ => {
                writeln!(
                    ::std::io::stderr(),
                    "Encountered geojson that is not a feature: {}",
                    g
                )
                .expect("Could not write to stderr");
            }
        }
    }

    let feature_collection = FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };

    let output = GeoJson::FeatureCollection(feature_collection);

    write!(::std::io::stdout(), "{}", output.to_string()).expect("Unable to write to stdout");

    Ok(())
}
