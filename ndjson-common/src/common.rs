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

use geo::{BoundingRect, Rect};
use geojson::Feature;
use std::convert::TryFrom;

pub fn calculate_bounding_box_if_not_exists(feat: &mut Feature) {
    if feat.bbox.is_none() {
        feat.bbox = feat
            .geometry
            .as_ref()
            .and_then(|g| geo::Geometry::<f64>::try_from(g).ok())
            .and_then(|g| g.bounding_rect())
            .map(bbox_from_rect);
    }
}

/// A GeoJSON bbox, `[minx, miny, maxx, maxy]`.
pub fn bbox_from_rect(rect: Rect<f64>) -> Vec<f64> {
    vec![rect.min().x, rect.min().y, rect.max().x, rect.max().y]
}

pub fn to_geo_json(geo_geometry: &geo::Geometry<f64>) -> geojson::GeometryValue {
    geojson::GeometryValue::from(geo_geometry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bbox_is_min_x_min_y_max_x_max_y() {
        let mut feat: Feature = r#"{"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[[[0,0],[2,0],[2,1],[0,1],[0,0]]]}}"#
            .parse()
            .expect("valid feature");
        calculate_bounding_box_if_not_exists(&mut feat);
        assert_eq!(feat.bbox, Some(vec![0.0, 0.0, 2.0, 1.0]));
    }

    #[test]
    fn test_existing_bbox_is_kept() {
        let mut feat: Feature = r#"{"type":"Feature","bbox":[1,2,3,4],"properties":{},"geometry":{"type":"Point","coordinates":[0,0]}}"#
            .parse()
            .expect("valid feature");
        calculate_bounding_box_if_not_exists(&mut feat);
        assert_eq!(feat.bbox, Some(vec![1.0, 2.0, 3.0, 4.0]));
    }
}
