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
use geojson_rstar::PointFeature;
use ndjson_common::error::NdJsonSpatialError;
use ndjson_common::ndjson::NdJsonGeojsonReader;
use rstar::{PointDistance, RTree};
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Write};

pub fn nearest_distance(mut reference_file: File) -> Result<(), NdJsonSpatialError> {
    let mut geojson_string = String::new();

    reference_file
        .read_to_string(&mut geojson_string)
        .map_err(|e| NdJsonSpatialError::Error(format!("Failed to read reference file: {}", e)))?;
    let json_data = geojson_string.parse::<GeoJson>().map_err(|e| {
        NdJsonSpatialError::Error(format!("Failed to parse reference geojson: {}", e))
    })?;
    if let GeoJson::FeatureCollection(feature_collection) = json_data {
        let point_features = feature_collection
            .features
            .into_iter()
            .filter_map(|point| point.try_into().ok())
            .collect::<Vec<PointFeature>>();
        let tree = RTree::bulk_load(point_features);

        for geojson in NdJsonGeojsonReader::default() {
            if let GeoJson::Feature(mut feature) = geojson? {
                if let geojson::Value::Point(ref point_vec) = feature
                    .geometry
                    .as_ref()
                    .ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Missing Geometry on feature, cannot compute distance".into(),
                        )
                    })?
                    .value
                {
                    let nearest = tree.nearest_neighbor(&[
                        *point_vec.get(0)
                            .ok_or_else(|| NdJsonSpatialError::Error("GeoJson point has less than 2 coordinates".into()))?, 
                        *point_vec.get(1)
                                .ok_or_else(|| NdJsonSpatialError::Error("GeoJson point has less than 2 coordinates".into()))?])
                                .ok_or_else(||
                        NdJsonSpatialError::Error("Missing nearest neighbor for point. Did reference file contain geojson points".to_string())
                    )?;
                    let distance = nearest.distance_2(&[
                        *point_vec.get(0).ok_or_else(|| {
                            NdJsonSpatialError::Error(
                                "GeoJson point has less than 2 coordinates".into(),
                            )
                        })?,
                        *point_vec.get(1).ok_or_else(|| {
                            NdJsonSpatialError::Error(
                                "GeoJson point has less than 2 coordinates".into(),
                            )
                        })?,
                    ]);

                    let number = serde_json::Number::from_f64(distance).ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Could not convert f64 to Json Number".to_string(),
                        )
                    })?;

                    feature.properties.as_mut().map(|p| {
                        p.insert("distance".to_string(), serde_json::Value::Number(number))
                    });

                    writeln!(::std::io::stdout(), "{}", feature.to_string())
                        .expect("Unable to write to stdout");
                }
            }
        }
    }
    Ok(())
}
