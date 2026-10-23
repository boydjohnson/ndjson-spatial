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
use geojson::{GeoJson, GeometryValue};
use ndjson_common::error::NdJsonSpatialError;
use rstar::primitives::{GeomWithData, Rectangle};
use std::{convert::TryFrom, fs::File, io::Read};

/// Reads a whole reference GeoJSON file.
pub fn read_geojson_file(mut reference_file: File) -> Result<GeoJson, NdJsonSpatialError> {
    let mut geojson_string = String::new();

    reference_file
        .read_to_string(&mut geojson_string)
        .map_err(|e| {
            NdJsonSpatialError::Error(format!("Error: reading from reference file {}", e))
        })?;
    let json_data = geojson_string
        .parse::<GeoJson>()
        .map_err(|e| NdJsonSpatialError::Error(format!("Error parsing geojson: {}", e)))?;
    Ok(json_data)
}

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
    let (rect, geos_geometry) = to_geos_with_rect(geometry)?;
    Some(GeomWithData::new(rect, geos_geometry))
}

/// A GeoJSON geometry as GEOS plus its bounding rectangle. `None` if it is
/// empty or GEOS can't represent it.
pub fn to_geos_with_rect(
    geometry: &geojson::Geometry,
) -> Option<(Rectangle<[f64; 2]>, geos::Geometry)> {
    let geo_geometry = geo::Geometry::<f64>::try_from(geometry).ok()?;
    let rect = geo_geometry.bounding_rect()?;
    let geos_geometry = geos::Geometry::try_from(&geo_geometry).ok()?;
    Some((
        Rectangle::from_corners(rect.min().x_y().into(), rect.max().x_y().into()),
        geos_geometry,
    ))
}
