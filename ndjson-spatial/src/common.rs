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

use gdal::{
    errors::Error,
    vector::{Geometry, ToGdal},
};
use geojson_rstar::Feature;

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
}

pub fn geojson_to_gdal(feature: &Feature) -> Result<Geometry, Error> {
    match feature {
        Feature::Point(p) => p.geo_point().to_gdal(),
        Feature::LineString(l) => l.geo_line().to_gdal(),
        Feature::Polygon(p) => p.geo_polygon().to_gdal(),
        Feature::MultiPoint(p) => p.geo_points().to_gdal(),
        Feature::MultiLineString(l) => l.geo_lines().to_gdal(),
        Feature::MultiPolygon(p) => p.geo_polygons().to_gdal(),
        Feature::GeometryCollection(g) => g.geo_geometry().to_gdal(),
    }
}
