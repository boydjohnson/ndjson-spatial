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

use crate::common::{index_geometry, read_geojson_file, GeometryType};
use geojson::GeoJson;
use geos::Geom;
use ndjson_common::{
    common::{calculate_bounding_box_if_not_exists, to_geo_json},
    error::NdJsonSpatialError,
    ndjson::NdJsonGeojsonReader,
};
use rstar::{RTree, RTreeObject};
use std::{convert::TryInto, fs::File, io::Write};

pub fn intersection(reference_file: File, geometry_type: &str) -> Result<(), NdJsonSpatialError> {
    let features = read_geojson_file(reference_file)?;
    let geometry_type = GeometryType::from_str(geometry_type).ok_or_else(|| {
        NdJsonSpatialError::Error(
            "geo-type must be one of point, line, polygon, multipoint, multiline, multipolygon"
                .to_string(),
        )
    })?;
    let tree = if let GeoJson::FeatureCollection(features) = features {
        let indexed = features
            .features
            .iter()
            .filter_map(|f| f.geometry.as_ref())
            .filter(|g| geometry_type.matches(&g.value))
            .filter_map(index_geometry)
            .collect();
        RTree::bulk_load(indexed)
    } else {
        return Err(NdJsonSpatialError::Error(
            "Reference file was not a feature collection.".to_string(),
        ));
    };

    for geojson in NdJsonGeojsonReader::default() {
        match geojson {
            Ok(geojson) => {
                if let GeoJson::Feature(feature) = geojson {
                    let incoming = match feature.geometry.as_ref().and_then(index_geometry) {
                        Some(incoming) => incoming,
                        None => {
                            writeln!(std::io::stderr(), "Error converting to geos types")
                                .expect("Unable to write to stderr");
                            continue;
                        }
                    };

                    let mut acc = vec![];

                    for inter in tree.locate_in_envelope_intersecting(incoming.envelope()) {
                        if let Ok(geos_geom) = incoming.data.intersection(&inter.data) {
                            let geo_geometry: geo::Geometry<f64> = geos_geom.try_into()?;

                            let mut feat = feature.clone();
                            feat.geometry =
                                Some(geojson::Geometry::new(to_geo_json(&geo_geometry)));
                            feat.bbox = None;
                            calculate_bounding_box_if_not_exists(&mut feat);
                            acc.push(feat);
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
