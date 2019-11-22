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

use geojson::GeoJson;
use ndjson_common::error::NdJsonSpatialError;
use ndjson_common::ndjson::NdJsonGeojsonReader;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::io::{BufRead, BufReader};

pub fn join(
    reference_file: File,
    reference_fields: Vec<String>,
    stream_fields: Vec<String>,
) -> Result<(), NdJsonSpatialError> {
    let reference_reader = BufReader::new(reference_file);

    let mut references = HashMap::new();

    for line in reference_reader.lines() {
        if let Ok(g) = line.unwrap().parse::<GeoJson>() {
            if let GeoJson::Feature(mut feat) = g {
                let prop = feat.properties.clone().unwrap();

                let field_values: Vec<String> = reference_fields
                    .iter()
                    .filter_map(|f| prop.get(f).cloned())
                    .filter_map(|v| {
                        if let serde_json::Value::String(s) = v {
                            Some(s)
                        } else {
                            None
                        }
                    })
                    .collect();

                references.insert(field_values, feat.properties.take().unwrap());
            }
        }
    }

    for geo in NdJsonGeojsonReader::default() {
        match geo {
            Ok(geo) => {
                if let GeoJson::Feature(mut feature) = geo {
                    let properties = feature.properties.as_mut().unwrap();

                    let s_fields: Vec<String> = stream_fields
                        .iter()
                        .filter_map(|f| properties.get(f))
                        .filter_map(|v| {
                            if let serde_json::Value::String(s) = v {
                                Some(s.to_owned())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if let Some(props) = references.get(&s_fields) {
                        for (k, v) in props.iter() {
                            properties.insert(k.to_owned(), v.to_owned());
                        }
                    }
                    let geojson_feature = GeoJson::Feature(feature);
                    if let Err(e) = writeln!(::std::io::stdout(), "{}", geojson_feature.to_string())
                    {
                        panic!("Error during writing to stdout: {}", e);
                    }
                }
            }
            Err(e) => {
                if writeln!(::std::io::stderr(), "{:?}", e).is_err() {
                    panic!(
                        "During reporting error, {:?}, could not write to std err",
                        e
                    );
                }
            }
        }
    }
    Ok(())
}
