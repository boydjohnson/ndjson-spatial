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

use crate::common::{geojson_to_gdal, GeometryType};
use gdal::vector::GeometryIntersection;
use geojson::GeoJson;
use geojson_rstar::Feature;
use ndjson_common::common::to_geo_json;
use ndjson_common::error::NdJsonSpatialError;
use ndjson_common::ndjson::NdJsonGeojsonReader;
use rstar::{RTree, RTreeObject};
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Write};

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

pub fn intersection(reference_file: File, geometry_type: &str) -> Result<(), NdJsonSpatialError> {
    let features = read_geojson_file(reference_file)?;
    let tree = if let GeoJson::FeatureCollection(features) = features {
        match GeometryType::from_str(geometry_type) {
            Some(GeometryType::Point) => {
                let points = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::Point)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(points)
            }
            Some(GeometryType::Line) => {
                let lines = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::LineString)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(lines)
            }
            Some(GeometryType::Polygon) => {
                let polygons = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::Polygon)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(polygons)
            }
            Some(GeometryType::MultiPoint) => {
                let m_points = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::MultiPoint)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(m_points)
            }
            Some(GeometryType::MultiLine) => {
                let m_lines = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::MultiLineString)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(m_lines)
            }
            None => {
                return Err(NdJsonSpatialError::Error(
                    "geo-type must be one of point, line, polygon, multipoint, multiline"
                        .to_string(),
                ))
            }
            Some(GeometryType::MultiPolygon) => {
                let m_polygons = features
                    .features
                    .into_iter()
                    .filter_map(|f| f.try_into().ok())
                    .map(Feature::MultiPolygon)
                    .collect::<Vec<Feature>>();
                RTree::bulk_load(m_polygons)
            }
        }
    } else {
        return Err(NdJsonSpatialError::Error(
            "Reference file was not a feature collection.".to_string(),
        ));
    };

    for geojson in NdJsonGeojsonReader::default() {
        match geojson {
            Ok(geojson) => {
                if let GeoJson::Feature(feature) = geojson {
                    let feat: Feature = feature
                        .try_into()
                        .map_err(|e| NdJsonSpatialError::Error(format!("Error {:?}", e)))?;

                    let incoming = geojson_to_gdal(&feat);

                    let mut acc = vec![];

                    for inter in tree.locate_in_envelope_intersecting(&feat.envelope()) {
                        let g = geojson_to_gdal(inter);

                        match (&incoming, &g) {
                            (Ok(first), Ok(other)) => {
                                if let Some(gdal_geom) = first.intersection(&other) {
                                    let geo_geometry: geo_types::Geometry<f64> = gdal_geom.into();

                                    let mut feat: geojson::Feature = feat.clone().into();

                                    let geo_json_value = to_geo_json(&geo_geometry);

                                    feat.geometry = Some(geojson::Geometry::new(geo_json_value));

                                    let feat: Feature = feat.try_into().map_err(|e| {
                                        NdJsonSpatialError::Error(format!(
                                            "Error converting from Geojson: {:?}",
                                            e
                                        ))
                                    })?;

                                    let feat: geojson::Feature = feat.into();
                                    acc.push(feat);
                                }
                            }
                            _ => {
                                writeln!(std::io::stderr(), "Error converting to gdal types")
                                    .expect("Unable to write to stderr");
                            }
                        }
                    }

                    for feat in acc {
                        if let Err(err) = writeln!(std::io::stdout(), "{}", feat) {
                            writeln!(std::io::stderr(), "Error writing to stdout: {}", err)
                                .expect("Unable to write to stderr");
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
