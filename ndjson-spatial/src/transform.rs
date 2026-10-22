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
    spatial_ref::{AxisMappingStrategy, CoordTransform, SpatialRef},
    vector::Geometry as GdalGeometry,
};
use ndjson_common::{
    common::calculate_bounding_box_if_not_exists, error::NdJsonSpatialError,
    ndjson::NdJsonGeojsonReader,
};
use std::io::{BufRead, Write};

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
        if let geojson::GeoJson::Feature(mut feat) =
            line.map_err(|e| NdJsonSpatialError::Error(format!("Error reading Geojson: {:?}", e)))?
        {
            let geometry = match feat.geometry.as_ref() {
                Some(g) => g,
                None => {
                    writeln!(
                        std::io::stderr(),
                        "Error converting from Geojson: feature has no geometry"
                    )
                    .expect("Unable to write to stderr");
                    continue;
                }
            };

            let mut gdal_geometry =
                GdalGeometry::from_geojson(&geometry.to_string()).map_err(|e| {
                    NdJsonSpatialError::Error(format!("Error converting to Gdal: {}", e))
                })?;

            gdal_geometry
                .transform_inplace(&crs_trans)
                .map_err(|e| NdJsonSpatialError::Error(format!("Error tranforming crs: {}", e)))?;

            let geometry = gdal_geometry
                .json()
                .map_err(|e| {
                    NdJsonSpatialError::Error(format!("Error converting from Gdal: {}", e))
                })?
                .parse::<geojson::GeoJson>()
                .map_err(|e| {
                    NdJsonSpatialError::Error(format!("Error reading Gdal geojson: {:?}", e))
                })?;

            let geometry = match geometry {
                geojson::GeoJson::Geometry(g) => g,
                other => {
                    return Err(NdJsonSpatialError::Error(format!(
                        "Expected a geometry from Gdal, got: {}",
                        other
                    )))
                }
            };

            feat.geometry = Some(geometry);
            // Any incoming bbox is in the source CRS.
            feat.bbox = None;
            calculate_bounding_box_if_not_exists(&mut feat);

            if let Err(e) = writeln!(writer, "{}", feat) {
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

    let mut f = f.map_err(|e| {
        NdJsonSpatialError::Error(format!("Error creating 'from' spatial ref: {}", e))
    })?;
    let mut t = t.map_err(|e| {
        NdJsonSpatialError::Error(format!("Error creating 'to' spatial_ref: {}", e))
    })?;

    // GDAL 3 defaults to the authority's axis order (lat/lon for EPSG:4326);
    // GeoJSON is always x/y, so keep the traditional GIS order.
    f.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);
    t.set_axis_mapping_strategy(AxisMappingStrategy::TraditionalGisOrder);

    CoordTransform::new(&f, &t)
        .map_err(|e| NdJsonSpatialError::Error(format!("Error creating tranform: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transform_point_to_web_mercator() {
        let input = r#"{"type":"Feature","properties":{"a":1},"geometry":{"type":"Point","coordinates":[-93.0,45.0]}}"#;
        let mut out = vec![];
        transform(
            input.as_bytes(),
            CrsSpecification::Epsg(4326),
            CrsSpecification::Epsg(3857),
            &mut out,
        )
        .expect("transform succeeds");
        let feat: geojson::Feature = std::str::from_utf8(&out)
            .expect("utf-8")
            .trim()
            .parse()
            .expect("valid feature");
        let bbox = feat.bbox.expect("bbox recomputed");
        assert!((bbox[0] - -10_352_712.64).abs() < 1.0, "{:?}", bbox);
        assert!((bbox[1] - 5_621_521.49).abs() < 1.0, "{:?}", bbox);
        assert_eq!(feat.properties.expect("properties")["a"], 1);
    }
}
