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

use geojson::{GeoJson, GeometryValue};
use ndjson_common::{error::NdJsonSpatialError, ndjson::NdJsonGeojsonReader};
use rstar::{PointDistance, RTree};
use std::{
    fs::File,
    io::{Read, Write},
};

fn point_coordinates(value: &GeometryValue) -> Option<[f64; 2]> {
    match value {
        GeometryValue::Point { coordinates } => match coordinates.as_slice() {
            [x, y] => Some([*x, *y]),
            _ => None,
        },
        _ => None,
    }
}

pub fn nearest_distance(mut reference_file: File) -> Result<(), NdJsonSpatialError> {
    let mut geojson_string = String::new();

    reference_file
        .read_to_string(&mut geojson_string)
        .map_err(|e| NdJsonSpatialError::Error(format!("Failed to read reference file: {}", e)))?;
    let json_data = geojson_string.parse::<GeoJson>().map_err(|e| {
        NdJsonSpatialError::Error(format!("Failed to parse reference geojson: {}", e))
    })?;
    if let GeoJson::FeatureCollection(feature_collection) = json_data {
        let points = feature_collection
            .features
            .iter()
            .filter_map(|f| f.geometry.as_ref())
            .filter_map(|g| point_coordinates(&g.value))
            .collect::<Vec<[f64; 2]>>();
        let tree = RTree::bulk_load(points);

        for geojson in NdJsonGeojsonReader::default() {
            if let GeoJson::Feature(mut feature) = geojson? {
                let value = &feature
                    .geometry
                    .as_ref()
                    .ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Missing Geometry on feature, cannot compute distance".into(),
                        )
                    })?
                    .value;
                if let GeometryValue::Point { coordinates } = value {
                    let point = match coordinates.as_slice() {
                        [x, y, ..] => [*x, *y],
                        _ => {
                            return Err(NdJsonSpatialError::Error(
                                "GeoJson point has less than 2 coordinates".into(),
                            ))
                        }
                    };
                    let nearest = tree.nearest_neighbor(point).ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Missing nearest neighbor for point. Did reference file contain geojson points".to_string(),
                        )
                    })?;
                    let distance = nearest.distance_2(&point);

                    let number = serde_json::Number::from_f64(distance).ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Could not convert f64 to Json Number".to_string(),
                        )
                    })?;

                    if let Some(p) = feature.properties.as_mut() {
                        p.insert("distance".to_string(), serde_json::Value::Number(number));
                    }

                    writeln!(::std::io::stdout(), "{}", feature)
                        .expect("Unable to write to stdout");
                }
            }
        }
    }
    Ok(())
}
