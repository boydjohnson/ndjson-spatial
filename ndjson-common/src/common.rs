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

use geo::{prelude::BoundingRect, Rect};
use geojson::{Feature, Value};
use geojson_rstar::conversion;

pub fn calculate_bounding_box_if_not_exists(feat: &mut Feature) {
    if feat.bbox.is_none() {
        let bbox = match feat.geometry.as_ref().map(|g| &g.value) {
            Some(Value::Point(p)) => {
                let point = conversion::create_geo_point(&p);
                Some(vec![point.lng(), point.lat(), point.lng(), point.lat()])
            }
            Some(Value::MultiPoint(mp)) => {
                let geo_multi_point = conversion::create_geo_multi_point(&mp);
                let bounding_rect = geo_multi_point.bounding_rect();
                do_calc_bounding_box(bounding_rect)
            }
            Some(Value::LineString(l)) => {
                let geo_linestring = conversion::create_geo_line_string(&l);
                let bounding_rect = geo_linestring.bounding_rect();
                do_calc_bounding_box(bounding_rect)
            }
            Some(Value::MultiLineString(ml)) => {
                let geo_multi_linestring = conversion::create_geo_multi_line_string(&ml);
                let bounding_rect = geo_multi_linestring.bounding_rect();
                do_calc_bounding_box(bounding_rect)
            }
            Some(Value::Polygon(p)) => {
                let geo_polygon = conversion::create_geo_polygon(&p);
                let bounding_rect = geo_polygon.bounding_rect();
                do_calc_bounding_box(bounding_rect)
            }
            Some(Value::MultiPolygon(mp)) => {
                let geo_multi_polygon = conversion::create_geo_multi_polygon(&mp);
                let bounding_rect = geo_multi_polygon.bounding_rect();
                do_calc_bounding_box(bounding_rect)
            }
            _ => None,
        };
        feat.bbox = bbox;
    }
}

fn do_calc_bounding_box(bounding_rect: Option<Rect<f64>>) -> Option<Vec<f64>> {
    if let Some(b) = bounding_rect {
        Some(vec![b.min().x, b.max().x, b.min().y, b.max().y])
    } else {
        None
    }
}

pub fn to_geo_json(geo_geometry: &geo::Geometry<f64>) -> geojson::Value {
    match geo_geometry {
        geo::Geometry::Point(p) => geojson::Value::from(p),
        geo::Geometry::LineString(l) => geojson::Value::from(l),
        geo::Geometry::Polygon(p) => geojson::Value::from(p),
        geo::Geometry::MultiPoint(p) => geojson::Value::from(p),
        geo::Geometry::MultiPolygon(p) => geojson::Value::from(p),
        geo::Geometry::MultiLineString(p) => geojson::Value::from(p),
        geo::Geometry::GeometryCollection(g) => geojson::Value::from(g),
        _ => panic!("Unsupported geometry type"),
    }
}
