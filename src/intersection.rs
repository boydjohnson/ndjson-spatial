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
use crate::ndjson::NdJsonGeojsonReader;
use geo_booleanop::boolean::BooleanOp;
use geojson::{GeoJson, Geometry};
use geojson_rstar::conversion::create_geo_polygon;
use geojson_rstar::PolygonFeature;
use rstar::{RTree, RTreeObject};
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Write};

pub fn read_geojson_file(
    mut reference_file: File,
) -> Result<Vec<PolygonFeature>, NdJsonSpatialError> {
    let mut geojson_string = String::new();

    reference_file
        .read_to_string(&mut geojson_string)
        .map_err(|e| {
            NdJsonSpatialError::Error(format!("Error: reading from reference file {}", e))
        })?;
    let json_data = geojson_string
        .parse::<GeoJson>()
        .map_err(|e| NdJsonSpatialError::Error(format!("Error parsing geojson: {}", e)))?;
    if let GeoJson::FeatureCollection(feature_collection) = json_data {
        Ok(feature_collection
            .features
            .into_iter()
            .filter_map(|polygon| polygon.try_into().ok())
            .collect::<Vec<PolygonFeature>>())
    } else {
        Err(NdJsonSpatialError::Error(
            "Json data is not feature collection".to_string(),
        ))
    }
}

pub fn intersection(reference_file: File) -> Result<(), NdJsonSpatialError> {
    let polygon_features = read_geojson_file(reference_file)?;
    let tree = RTree::bulk_load(polygon_features);

    for geojson in NdJsonGeojsonReader::default() {
        match geojson {
            Ok(geojson) => {
                if let GeoJson::Feature(feature) = geojson {
                    let mut polygon: PolygonFeature = feature
                        .try_into()
                        .map_err(|e| NdJsonSpatialError::Error(format!("Error {:?}", e)))?;
                    let mut acc = vec![];
                    for p in tree.locate_in_envelope_intersecting(&polygon.envelope()) {
                        let inter = BooleanOp::<f64>::intersection(
                            &create_geo_polygon(p.polygon()),
                            &create_geo_polygon(polygon.polygon()),
                        );
                        if !inter.0.is_empty() {
                            acc.push((inter, polygon.properties.take()));
                        }
                    }
                    let features = acc
                        .into_iter()
                        .map(|(m, p)| ((&m).into(), p))
                        .map(|(v, p)| (Geometry::new(v), p))
                        .map(|(g, p)| geojson::Feature {
                            bbox: None,
                            geometry: Some(g),
                            id: None,
                            properties: p,
                            foreign_members: None,
                        })
                        .collect::<Vec<geojson::Feature>>();
                    if !features.is_empty() {
                        for feature in features {
                            let f = GeoJson::Feature(feature);
                            writeln!(::std::io::stdout(), "{}", f.to_string())
                                .expect("Error writing to stdout");
                        }
                    }
                }
            }
            Err(e) => {
                if let Err(err) = writeln!(::std::io::stderr(), "{:?}", e) {
                    panic!("Error reporting error, {}, could not write to std err", err);
                }
            }
        }
    }

    Ok(())
}
