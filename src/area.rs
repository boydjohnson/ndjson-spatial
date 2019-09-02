extern crate geo;
extern crate geojson;
extern crate serde_json;

use crate::error::NdJsonSpatialError;
use crate::ndjson::NdJsonGeojsonReader;
use geo::algorithm::area::Area;
use geojson_rstar::conversion::{create_geo_multi_polygon, create_geo_polygon};
use geojson_rstar::{multipolygon_feature::MultiPolygonFeature, PolygonFeature};
use std::convert::TryInto;
use std::io::{BufRead, BufReader};
use std::io::{Stdin, Write};

pub struct NdjsonSpatialArea<IN> {
    std_in: IN,
}

impl Default for NdjsonSpatialArea<BufReader<Stdin>> {
    fn default() -> Self {
        NdjsonSpatialArea {
            std_in: BufReader::new(std::io::stdin()),
        }
    }
}

impl<IN> NdjsonSpatialArea<IN> {
    fn new(std_in: IN) -> Self {
        NdjsonSpatialArea { std_in }
    }
}

impl<IN> NdjsonSpatialArea<IN>
where
    IN: BufRead,
{
    pub fn area(self, field_name: String) -> Result<(), NdJsonSpatialError> {
        for geo in NdJsonGeojsonReader::new(self.std_in) {
            if let Ok(geojson::GeoJson::Feature(feat)) = geo {
                let area = if let Ok(p) = feat.clone().try_into() {
                    let mut poly: PolygonFeature = p;

                    create_geo_polygon(poly.polygon()).area()
                } else if let Ok(p) = feat.clone().try_into() {
                    let poly: MultiPolygonFeature = p;
                    create_geo_multi_polygon(poly.polygons()).area()
                };

                feat.properties.as_mut().map(|p| {
                    p.insert(
                        field_name.clone(),
                        serde_json::Value::Number(serde_json::Number::from_f64(area).map_err(
                            |e| {
                                NdJsonSpatialError::Error(format!(
                                    "Error converting f64 to Json Number: {}",
                                    e
                                ))
                            },
                        )?),
                    )
                });
                writeln!(::std::io::stdout(), "{}", feat.to_string())
                    .expect("Unable to write to stdout");
            }
        }
        Ok(())
    }
}
