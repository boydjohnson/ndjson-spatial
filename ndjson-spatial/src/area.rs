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

use geojson::Value;
use geos::{Geom, Geometry};
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
    pub fn area(&mut self, field_name: String, bbox: bool) -> Result<(), NdJsonSpatialError> {
        for geo in NdJsonGeojsonReader::new(&mut self.std_in) {
            if let Ok(geojson::GeoJson::Feature(mut feat)) = geo {
                let area = match feat.geometry.as_ref() {
                    Some(geometry) => match geometry.value {
                        Value::MultiPolygon(_) | Value::Polygon(_) => {
                            let geos_geometry: Geometry = geometry.clone().try_into()?;
                            geos_geometry.area()?
                        }
                        Value::Point(_) => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type Point not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        Value::MultiPoint(_) => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type MultiPoint not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        Value::LineString(_) => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type LineString not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        Value::MultiLineString(_) => {
                            return Err(
                                NdJsonSpatialError::Error(
                                    "Area got called on Geojson Feature that was of type MultiLineString not polygon or multipolygon.".to_string()
                                )
                            );
                        }
                        Value::GeometryCollection(_) => {
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

                writeln!(self.std_out, "{}", feat.to_string()).expect("Unable to write to stdout");
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
            .area("Area".to_string(), true)
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
}
