extern crate geo;
extern crate geojson;
extern crate serde_json;

use crate::error::NdJsonSpatialError;
use crate::ndjson::NdJsonReader;
use geo::algorithm::area::Area;
use geojson_rstar::conversion::create_geo_polygon;
use geojson_rstar::PolygonFeature;
use std::convert::TryInto;
use std::io::Write;

pub fn area(field_name: String) -> Result<(), NdJsonSpatialError> {
    for geo in NdJsonReader::default() {
        if let Ok(geojson::GeoJson::Feature(feat)) = geo {
            if let Ok(p) = feat.try_into() {
                let mut poly: PolygonFeature = p;

                let a: f64 = create_geo_polygon(poly.polygon()).area();
                poly.properties.as_mut().map(|p| {
                    p.insert(
                        field_name.clone(),
                        serde_json::Value::Number(serde_json::Number::from_f64(a).unwrap()),
                    )
                });

                let v = geojson::Value::Polygon(poly.polygon().to_owned());
                let g = geojson::Geometry::new(v);
                let feature = geojson::GeoJson::Feature(geojson::Feature {
                    id: poly.id,
                    bbox: None,
                    geometry: Some(g),
                    properties: poly.properties,
                    foreign_members: poly.foreign_members,
                });

                writeln!(::std::io::stdout(), "{}", feature.to_string())
                    .expect("Unable to write to stdout");
            }
        }
    }
    Ok(())
}
