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

use crate::functions::geodesic_area;
use geo::Area;
use geojson::GeometryValue;
use ndjson_common::{
    common::calculate_bounding_box_if_not_exists, error::NdJsonSpatialError,
    ndjson::NdJsonGeojsonReader,
};
use serde_json::Map;
use std::{
    convert::TryInto,
    io::{BufRead, BufReader, BufWriter, Stdin, Stdout, Write},
};

pub struct NdjsonSpatialArea<IN, OUT> {
    std_in: IN,
    std_out: OUT,
}

impl Default for NdjsonSpatialArea<BufReader<Stdin>, BufWriter<Stdout>> {
    fn default() -> Self {
        NdjsonSpatialArea::new(
            BufReader::new(std::io::stdin()),
            BufWriter::new(std::io::stdout()),
        )
    }
}

impl<IN, OUT> NdjsonSpatialArea<IN, OUT> {
    fn new(std_in: IN, std_out: OUT) -> Self {
        NdjsonSpatialArea { std_in, std_out }
    }
}

impl<IN, OUT> NdjsonSpatialArea<IN, OUT>
where
    IN: BufRead,
    OUT: Write,
{
    /// Adds the area of each feature's polygon or multipolygon as a
    /// property: square meters on the WGS84 ellipsoid, or with `planar`,
    /// square units of the coordinates.
    pub fn area(
        &mut self,
        field_name: String,
        bbox: bool,
        planar: bool,
    ) -> Result<(), NdJsonSpatialError> {
        for geo in NdJsonGeojsonReader::new(&mut self.std_in).flatten() {
            if let geojson::GeoJson::Feature(mut feat) = geo {
                let area = match feat.geometry.as_ref() {
                    Some(geometry) => match geometry.value {
                        GeometryValue::MultiPolygon { .. } | GeometryValue::Polygon { .. } => {
                            let geo_geometry: geo::Geometry<f64> = geometry.try_into()?;
                            if planar {
                                geo_geometry.unsigned_area()
                            } else {
                                geodesic_area(&geo_geometry)
                            }
                        }
                        GeometryValue::Point { .. } => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type Point not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        GeometryValue::MultiPoint { .. } => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type MultiPoint not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        GeometryValue::LineString { .. } => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type LineString not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        GeometryValue::MultiLineString { .. } => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type MultiLineString not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        GeometryValue::GeometryCollection { .. } => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type GeometryCollection not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                    },
                    None => 0.0,
                };

                let a = serde_json::Number::from_f64(area)
                    .ok_or_else(|| {
                        NdJsonSpatialError::Error("Error converting f64 to Json number".to_string())
                    })
                    .map(serde_json::Value::Number)?;

                feat.properties
                    .get_or_insert_with(Map::new)
                    .insert(field_name.clone(), a);

                if bbox {
                    calculate_bounding_box_if_not_exists(&mut feat);
                }

                writeln!(self.std_out, "{}", feat).expect("Unable to write to stdout");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geojson::GeoJson;

    #[test]
    fn test_area_polygon_success() {
        let out = vec![];

        let mut area_calc = NdjsonSpatialArea::<&[u8], Vec<u8>>::new(
            "{ \"type\": \"Feature\", \"properties\": { \"STATEFP\": 27 }, \"geometry\": { \"type\": \"Polygon\", \"coordinates\": [[[189776.5420303712, 4816290.5053447075] ,[761661.7830505947, 4816290.5053447075],[ 761661.7830505947, 5472415.100443922], [189776.5420303712, 5472415.100443922]]] }}".as_bytes(),
            out,
        );

        area_calc
            .area("Area".to_string(), true, true)
            .expect("Able to calculate area");
        let data =
            std::str::from_utf8(&area_calc.std_out).expect("Some of the bytes were not utf-8");
        let output = data
            .parse::<GeoJson>()
            .expect("The output was not valid geojson");
        if let GeoJson::Feature(feat) = output {
            assert!(feat.properties.is_some());

            let properties = feat.properties.expect("Properties is some");
            assert!(properties.contains_key("Area"));
        } else {
            panic!("Geojson was not a feature");
        }
    }

    fn area_of(line: &str, planar: bool) -> f64 {
        let mut area_calc = NdjsonSpatialArea::<&[u8], Vec<u8>>::new(line.as_bytes(), vec![]);
        area_calc
            .area("area".to_string(), false, planar)
            .expect("Able to calculate area");
        let out: serde_json::Value =
            serde_json::from_slice(&area_calc.std_out).expect("valid json");
        out["properties"]["area"].as_f64().expect("a number")
    }

    #[test]
    fn test_area_is_geodesic_by_default() {
        // one degree square on the equator, written clockwise
        let line = r#"{"type": "Feature", "properties": {}, "geometry": {"type": "Polygon", "coordinates": [[[0, 0], [0, 1], [1, 1], [1, 0], [0, 0]]]}}"#;
        let a = area_of(line, false);
        assert!((a - 12_308_778_361.0).abs() < 1e7, "{}", a);
        assert_eq!(area_of(line, true), 1.0);
    }
}
