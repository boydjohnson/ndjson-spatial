use crate::error::NdJsonSpatialError;
use crate::ndjson::NdJsonGeojsonReader;
use geojson::{FeatureCollection, GeoJson};
use std::io::Write;

pub fn to_geojson() -> Result<(), NdJsonSpatialError> {
    let mut features = vec![];
    for g in NdJsonGeojsonReader::default() {
        let g = g?;
        match g {
            GeoJson::Feature(f) => {
                features.push(f);
            }
            _ => {
                writeln!(
                    ::std::io::stderr(),
                    "Encountered geojson that is not a feature: {}",
                    g
                )
                .expect("Could not write to stderr");
            }
        }
    }

    let feature_collection = FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };

    let output = GeoJson::FeatureCollection(feature_collection);

    write!(::std::io::stdout(), "{}", output.to_string()).expect("Unable to write to stdout");

    Ok(())
}
