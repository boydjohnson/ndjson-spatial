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

use geo::BoundingRect;
use geojson::GeometryValue;
use rstar::primitives::{GeomWithData, Rectangle};
use std::convert::TryFrom;

pub enum GeometryType {
    Point,
    Line,
    Polygon,
    MultiPoint,
    MultiLine,
    MultiPolygon,
}

impl GeometryType {
    pub fn from_str(string: &str) -> Option<Self> {
        match string {
            "point" => Some(GeometryType::Point),
            "line" => Some(GeometryType::Line),
            "polygon" => Some(GeometryType::Polygon),
            "multipoint" => Some(GeometryType::MultiPoint),
            "multiline" => Some(GeometryType::MultiLine),
            "multipolygon" => Some(GeometryType::MultiPolygon),
            _ => None,
        }
    }

    pub fn matches(&self, value: &GeometryValue) -> bool {
        matches!(
            (self, value),
            (GeometryType::Point, GeometryValue::Point { .. })
                | (GeometryType::Line, GeometryValue::LineString { .. })
                | (GeometryType::Polygon, GeometryValue::Polygon { .. })
                | (GeometryType::MultiPoint, GeometryValue::MultiPoint { .. })
                | (
                    GeometryType::MultiLine,
                    GeometryValue::MultiLineString { .. }
                )
                | (
                    GeometryType::MultiPolygon,
                    GeometryValue::MultiPolygon { .. }
                )
        )
    }
}

/// A GEOS geometry stored in an `RTree` under its bounding rectangle.
pub type IndexedGeometry = GeomWithData<Rectangle<[f64; 2]>, geos::Geometry>;

/// Converts a GeoJSON geometry for indexing. `None` if it is empty or
/// GEOS can't represent it.
pub fn index_geometry(geometry: &geojson::Geometry) -> Option<IndexedGeometry> {
    let geo_geometry = geo::Geometry::<f64>::try_from(geometry).ok()?;
    let rect = geo_geometry.bounding_rect()?;
    let geos_geometry = geos::Geometry::try_from(&geo_geometry).ok()?;
    Some(GeomWithData::new(
        Rectangle::from_corners(rect.min().x_y().into(), rect.max().x_y().into()),
        geos_geometry,
    ))
}
