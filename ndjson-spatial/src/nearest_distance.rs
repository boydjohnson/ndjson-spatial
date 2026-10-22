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

/// Euclidean distance from `point` to the nearest point in `tree`.
fn nearest_point_distance(tree: &RTree<[f64; 2]>, point: [f64; 2]) -> Option<f64> {
    tree.nearest_neighbor(point)
        .map(|nearest| nearest.distance_2(&point).sqrt())
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
                    let distance = nearest_point_distance(&tree, point).ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Missing nearest neighbor for point. Did reference file contain geojson points".to_string(),
                        )
                    })?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nearest_point_distance_is_not_squared() {
        let tree = RTree::bulk_load(vec![[0.0, 0.0], [10.0, 10.0]]);
        assert_eq!(nearest_point_distance(&tree, [3.0, 4.0]), Some(5.0));
    }

    #[test]
    fn test_nearest_point_distance_empty_tree() {
        let tree: RTree<[f64; 2]> = RTree::new();
        assert_eq!(nearest_point_distance(&tree, [3.0, 4.0]), None);
    }
}
