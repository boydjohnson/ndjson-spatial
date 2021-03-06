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

use crate::common::geojson_to_gdal;
use gdal::spatial_ref::{CoordTransform, SpatialRef};
use geo::Geometry;
use geojson_rstar::Feature;
use ndjson_common::{common::to_geo_json, error::NdJsonSpatialError, ndjson::NdJsonGeojsonReader};
use std::{
    convert::TryInto,
    io::{BufRead, Write},
};

pub enum CrsSpecification {
    Epsg(u32),
    Proj(String),
}

pub fn transform<R: BufRead, W: Write>(
    reader: R,
    from: CrsSpecification,
    to: CrsSpecification,
    writer: &mut W,
) -> Result<(), NdJsonSpatialError> {
    let reader = NdJsonGeojsonReader::new(reader);

    let crs_trans = create_transform(&from, &to)?;

    for line in reader {
        if let geojson::GeoJson::Feature(feature) =
            line.map_err(|e| NdJsonSpatialError::Error(format!("Error reading Geojson: {:?}", e)))?
        {
            let feat: Feature = match feature.try_into().map_err(|e| {
                NdJsonSpatialError::Error(format!("Error converting from Geojson: {:?}", e))
            }) {
                Ok(f) => f,
                Err(err) => {
                    writeln!(std::io::stderr(), "{:?}", err).expect("Unable to write to stderr");
                    continue;
                }
            };
            let gdal_geometry = geojson_to_gdal(&feat);

            let mut gdal_geometry = gdal_geometry.map_err(|e| {
                NdJsonSpatialError::Error(format!("Error converting to Gdal: {}", e))
            })?;

            gdal_geometry
                .transform_inplace(&crs_trans)
                .map_err(|e| NdJsonSpatialError::Error(format!("Error tranforming crs: {}", e)))?;

            let geo_geometry: Geometry<f64> = gdal_geometry.into();

            let mut feat: geojson::Feature = feat.into();

            let geo_json_value = to_geo_json(&geo_geometry);

            feat.geometry = Some(geojson::Geometry::new(geo_json_value));

            let feat: Feature = feat.try_into().map_err(|e| {
                NdJsonSpatialError::Error(format!("Error converting from Geojson: {:?}", e))
            })?;

            let feat: geojson::Feature = feat.into();

            if let Err(e) = writeln!(writer, "{}", feat.to_string()) {
                writeln!(std::io::stderr(), "{}", e).expect("Unable to write to stderr");
            }
        }
    }
    Ok(())
}

fn create_transform(
    from: &CrsSpecification,
    to: &CrsSpecification,
) -> Result<CoordTransform, NdJsonSpatialError> {
    let f = match from {
        CrsSpecification::Epsg(epsg) => SpatialRef::from_epsg(*epsg),
        CrsSpecification::Proj(ref proj4) => SpatialRef::from_proj4(proj4),
    };
    let t = match to {
        CrsSpecification::Epsg(ref epsg) => SpatialRef::from_epsg(*epsg),
        CrsSpecification::Proj(ref proj4) => SpatialRef::from_proj4(proj4),
    };

    let f = f.map_err(|e| {
        NdJsonSpatialError::Error(format!("Error creating 'from' spatial ref: {}", e))
    })?;
    let t = t.map_err(|e| {
        NdJsonSpatialError::Error(format!("Error creating 'to' spatial_ref: {}", e))
    })?;

    CoordTransform::new(&f, &t)
        .map_err(|e| NdJsonSpatialError::Error(format!("Error creating tranform: {}", e)))
}
