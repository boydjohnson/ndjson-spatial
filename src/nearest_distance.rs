extern crate geo;
extern crate geojson;
extern crate geojson_rstar;
extern crate rstar;
extern crate serde_json;

use crate::error::NdJsonSpatialError;
use crate::ndjson::NdJsonReader;
use geojson::GeoJson;
use geojson_rstar::PointFeature;
use rstar::{PointDistance, RTree};
use serde_json::Value;
use std::convert::TryInto;
use std::fs::File;
use std::io::{Read, Write};

pub fn nearest_distance(
    mut reference_file: File,
    property: String,
) -> Result<(), NdJsonSpatialError> {
    let mut geojson_string = String::new();

    reference_file
        .read_to_string(&mut geojson_string)
        .map_err(|e| NdJsonSpatialError::Error(format!("Failed to read reference file: {}", e)))?;
    let json_data = geojson_string.parse::<GeoJson>().map_err(|e| {
        NdJsonSpatialError::Error(format!("Failed to parse reference geojson: {}", e))
    })?;
    if let GeoJson::FeatureCollection(feature_collection) = json_data {
        let point_features = feature_collection
            .features
            .into_iter()
            .filter_map(|point| point.try_into().ok())
            .collect::<Vec<PointFeature>>();
        let tree = RTree::bulk_load(point_features);

        for geojson in NdJsonReader::default() {
            if let GeoJson::Feature(feature) = geojson? {
                let prop = if let Some(Some(Value::String(prop))) =
                    feature.properties.map(|p| p.get(&property).cloned())
                {
                    Some(prop)
                } else {
                    None
                };

                if let geojson::Value::Point(point_vec) = feature
                    .geometry
                    .ok_or_else(|| {
                        NdJsonSpatialError::Error(
                            "Missing Geometry on feature, cannot compute distance".into(),
                        )
                    })?
                    .value
                {
                    let nearest = tree.nearest_neighbor(&[*point_vec.get(0).unwrap(), *point_vec.get(1).unwrap()]).ok_or({
                        NdJsonSpatialError::Error("Missing nearest neighbor for point. Did reference file contain geojson points".to_string())
                    })?;
                    let distance = nearest
                        .distance_2(&[*point_vec.get(0).unwrap(), *point_vec.get(1).unwrap()]);

                    writeln!(
                        ::std::io::stdout(),
                        "{{ \"distance\": {}, \"{}\": \"{}\" }}",
                        distance,
                        property,
                        prop.unwrap()
                    )
                    .expect("Unable to write to stdout");
                }
            }
        }
    }
    Ok(())
}
