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
use geo::algorithm::centroid::Centroid;
use geo::Point;
use geojson::{Feature, GeoJson, Geometry};
use std::convert::TryInto;
use std::io::Write;

pub fn compute_centroid() {
    for geojson in NdJsonGeojsonReader::default() {
        match geojson {
            Ok(geojson) => {
                let geo_json;
                match calculate_centroid_from_geojson(geojson) {
                    Ok((mut feat, point)) => {
                        let p: geojson::Value = (&point).into();
                        let g = Geometry::new(p);
                        feat.geometry.replace(g);
                        geo_json = GeoJson::Feature(feat);
                    }
                    Err(e) => {
                        writeln!(::std::io::stderr(), "{:?}", e)
                            .expect("Unable to write to stderr");
                        break;
                    }
                }

                writeln!(::std::io::stdout(), "{}", geo_json.to_string())
                    .expect("Unable to write to stdout");
            }
            Err(e) => {
                if let Err(err) = writeln!(::std::io::stderr(), "{:?}", e) {
                    panic!("Error reporting error, {}, could not write to stderr", err);
                }
            }
        }
    }
}

fn calculate_centroid_from_geojson(
    geojson: GeoJson,
) -> Result<(Feature, Point<f64>), NdJsonSpatialError> {
    if let GeoJson::Feature(feat) = geojson {
        let geometry = feat.geometry.as_ref();
        if geometry.is_none() {
            return Err(NdJsonSpatialError::Error(
                "Geometry missing from feature".to_string(),
            ));
        }

        let geometry = geometry.cloned().expect("already checked if None");

        match geometry.value {
            geojson::Value::MultiPolygon(multi) => {
                let p: geo_types::MultiPolygon<f64> = geojson::Value::MultiPolygon(multi).try_into().expect("failed to convert geojson");
                p.centroid().ok_or_else(|| {
                    NdJsonSpatialError::Error("Unable to compute centroid".to_string())
                }).map(|p| (feat, p))
            },
            geojson::Value::Polygon(poly) => {
                let p: geo_types::Polygon<f64> = geojson::Value::Polygon(poly).try_into().expect("failed to convert geojson");
                p.centroid().ok_or_else(|| {
                    NdJsonSpatialError::Error("Unable to compute centroid".to_string())
                }).map(|p| (feat, p))
            },
            _ => Err(NdJsonSpatialError::Error(
                "Error in ndjson geometry type, must be Polygon or Multipolygon for centroid measurement".to_string()))
        }
    } else {
        Err(NdJsonSpatialError::Error(
            "Invalid ndjson, expected single feature".to_string(),
        ))
    }
}
