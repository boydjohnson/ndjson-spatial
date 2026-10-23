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

//! `join-contains --ref`: a containment join of the stream against a
//! reference FeatureCollection held in an R-tree.
//!
//! By default each stream feature is joined with the reference features its
//! geometry contains. With `--within` the direction flips: each stream
//! feature is joined with the reference features that contain it, which is
//! the usual point-in-polygon join (points on stdin, polygons in `--ref`).
//!
//! Inner and `--left` joins write the matching reference features'
//! properties as an array into the stream feature's properties under
//! `--field-name`. `--semi` and `--anti` write stream features unchanged.

use crate::common::{read_geojson_file, to_geos_with_rect};
use geojson::{Feature, GeoJson, JsonObject, JsonValue};
use geos::Geom;
use ndjson_common::{error::NdJsonSpatialError, ndjson::NdJsonGeojsonReader};
use rstar::{
    primitives::{GeomWithData, Rectangle},
    Envelope, RTree, RTreeObject,
};
use std::{
    fs::File,
    io::{BufRead, Write},
};

/// Which stream features are written, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// Features with at least one match, with the matches added.
    Inner,
    /// Every feature, with its matches added (an empty array when none).
    Left,
    /// Features with at least one match, unchanged.
    Semi,
    /// Features without a match, unchanged.
    Anti,
}

/// Which side's geometry contains the other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The stream geometry contains the reference geometry.
    Contains,
    /// The stream geometry is within the reference geometry.
    Within,
}

pub struct JoinOptions {
    pub kind: JoinKind,
    pub direction: Direction,
    pub field_name: String,
}

/// Counts reported after a run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct JoinSummary {
    /// Reference features skipped because they have no usable geometry.
    pub skipped_reference: u64,
    /// Stream lines skipped: not a Feature, no usable geometry, or a GEOS
    /// error while testing containment.
    pub skipped_stream: u64,
    /// The first stream error: 1-based line number and message.
    pub first_error: Option<(u64, String)>,
}

struct Reference {
    geometry: geos::Geometry,
    properties: JsonValue,
}

type Indexed = GeomWithData<Rectangle<[f64; 2]>, usize>;

pub fn join_contains(
    reference_file: File,
    options: &JoinOptions,
    input: impl BufRead,
    output: &mut impl Write,
) -> Result<JoinSummary, NdJsonSpatialError> {
    let features = match read_geojson_file(reference_file)? {
        GeoJson::FeatureCollection(collection) => collection.features,
        _ => {
            return Err(NdJsonSpatialError::Error(
                "Reference file was not a feature collection.".to_string(),
            ))
        }
    };
    join_features(features, options, input, output)
}

fn join_features(
    features: Vec<Feature>,
    options: &JoinOptions,
    input: impl BufRead,
    output: &mut impl Write,
) -> Result<JoinSummary, NdJsonSpatialError> {
    let mut summary = JoinSummary::default();
    let mut references = vec![];
    let mut indexed = vec![];
    for feature in features {
        match feature.geometry.as_ref().and_then(to_geos_with_rect) {
            Some((rect, geometry)) => {
                indexed.push(GeomWithData::new(rect, references.len()));
                references.push(Reference {
                    geometry,
                    properties: feature
                        .properties
                        .map_or(JsonValue::Null, JsonValue::Object),
                });
            }
            None => summary.skipped_reference += 1,
        }
    }
    let tree: RTree<Indexed> = RTree::bulk_load(indexed);

    for (line, geojson) in (1..).zip(NdJsonGeojsonReader::new(input)) {
        let mut fail = |message: String| {
            summary.skipped_stream += 1;
            summary.first_error.get_or_insert((line, message));
        };
        let mut feature = match geojson {
            Ok(GeoJson::Feature(feature)) => feature,
            Ok(_) => {
                fail("not a Feature".to_string());
                continue;
            }
            Err(NdJsonSpatialError::Error(e)) => {
                fail(e);
                continue;
            }
        };
        let (rect, geometry) = match feature.geometry.as_ref().and_then(to_geos_with_rect) {
            Some(g) => g,
            None => {
                fail("missing or empty geometry".to_string());
                continue;
            }
        };

        let matches = match find_matches(&tree, &references, rect, &geometry, options) {
            Ok(matches) => matches,
            Err(e) => {
                fail(format!("{}", e));
                continue;
            }
        };

        let write = match options.kind {
            JoinKind::Inner => !matches.is_empty(),
            JoinKind::Left => true,
            JoinKind::Semi => !matches.is_empty(),
            JoinKind::Anti => matches.is_empty(),
        };
        if !write {
            continue;
        }
        if matches!(options.kind, JoinKind::Inner | JoinKind::Left) {
            let joined = matches
                .into_iter()
                .map(|i| references[i].properties.clone())
                .collect();
            feature
                .properties
                .get_or_insert_with(JsonObject::new)
                .insert(options.field_name.clone(), JsonValue::Array(joined));
        }
        if let Err(e) = writeln!(output, "{}", feature) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                break;
            }
            return Err(NdJsonSpatialError::Error(format!(
                "Error writing to stdout: {}",
                e
            )));
        }
    }
    output
        .flush()
        .map_err(|e| NdJsonSpatialError::Error(format!("Error writing to stdout: {}", e)))?;
    Ok(summary)
}

/// Indices of the reference features that match `geometry`, in reference
/// file order.
fn find_matches(
    tree: &RTree<Indexed>,
    references: &[Reference],
    rect: Rectangle<[f64; 2]>,
    geometry: &geos::Geometry,
    options: &JoinOptions,
) -> Result<Vec<usize>, geos::Error> {
    let envelope = rect.envelope();
    let candidates: Vec<usize> = match options.direction {
        // a contained geometry's box lies inside the container's box
        Direction::Contains => tree.locate_in_envelope(envelope).map(|c| c.data).collect(),
        Direction::Within => tree
            .locate_in_envelope_intersecting(envelope)
            .filter(|c| c.envelope().contains_envelope(&envelope))
            .map(|c| c.data)
            .collect(),
    };
    let mut matches = vec![];
    for i in candidates {
        let reference = &references[i].geometry;
        let hit = match options.direction {
            Direction::Contains => geometry.contains(reference)?,
            Direction::Within => reference.contains(geometry)?,
        };
        if hit {
            matches.push(i);
        }
    }
    matches.sort_unstable();
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn feature(geometry: Value, properties: Value) -> Feature {
        serde_json::from_value(json!({
            "type": "Feature",
            "geometry": geometry,
            "properties": properties,
        }))
        .unwrap()
    }

    fn square(x: f64, y: f64, size: f64) -> Value {
        json!({"type": "Polygon", "coordinates": [[
            [x, y], [x + size, y], [x + size, y + size], [x, y + size], [x, y]
        ]]})
    }

    fn point(x: f64, y: f64) -> Value {
        json!({"type": "Point", "coordinates": [x, y]})
    }

    fn run(
        reference: Vec<Feature>,
        stream: &[Feature],
        kind: JoinKind,
        direction: Direction,
    ) -> (Vec<Value>, JoinSummary) {
        let input = stream
            .iter()
            .map(|f| format!("{}\n", f))
            .collect::<String>();
        let options = JoinOptions {
            kind,
            direction,
            field_name: "matches".to_string(),
        };
        let mut out = vec![];
        let summary = join_features(reference, &options, input.as_bytes(), &mut out).unwrap();
        let lines = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (lines, summary)
    }

    fn names(lines: &[Value]) -> Vec<Value> {
        lines
            .iter()
            .map(|l| l["properties"]["name"].clone())
            .collect()
    }

    fn polygons() -> Vec<Feature> {
        vec![
            feature(square(0.0, 0.0, 10.0), json!({"zone": "a"})),
            feature(square(5.0, 5.0, 10.0), json!({"zone": "b"})),
            feature(square(100.0, 100.0, 1.0), json!({"zone": "c"})),
        ]
    }

    fn points() -> Vec<Feature> {
        vec![
            feature(point(1.0, 1.0), json!({"name": "only a"})),
            feature(point(7.0, 7.0), json!({"name": "a and b"})),
            feature(point(50.0, 50.0), json!({"name": "none"})),
        ]
    }

    #[test]
    fn test_within_inner_is_point_in_polygon() {
        let (lines, summary) = run(polygons(), &points(), JoinKind::Inner, Direction::Within);
        assert_eq!(names(&lines), vec![json!("only a"), json!("a and b")]);
        assert_eq!(lines[0]["properties"]["matches"], json!([{"zone": "a"}]));
        assert_eq!(
            lines[1]["properties"]["matches"],
            json!([{"zone": "a"}, {"zone": "b"}])
        );
        assert_eq!(summary, JoinSummary::default());
    }

    #[test]
    fn test_within_left_keeps_unmatched() {
        let (lines, _) = run(polygons(), &points(), JoinKind::Left, Direction::Within);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2]["properties"]["matches"], json!([]));
    }

    #[test]
    fn test_semi_and_anti_write_features_unchanged() {
        let (semi, _) = run(polygons(), &points(), JoinKind::Semi, Direction::Within);
        assert_eq!(names(&semi), vec![json!("only a"), json!("a and b")]);
        assert!(semi[0]["properties"].get("matches").is_none());

        let (anti, _) = run(polygons(), &points(), JoinKind::Anti, Direction::Within);
        assert_eq!(names(&anti), vec![json!("none")]);
        assert!(anti[0]["properties"].get("matches").is_none());
    }

    #[test]
    fn test_contains_joins_reference_points_into_polygons() {
        let reference = points();
        let stream = vec![
            feature(square(0.0, 0.0, 10.0), json!({"name": "big"})),
            feature(square(6.0, 6.0, 2.0), json!({"name": "small"})),
            feature(square(20.0, 20.0, 1.0), json!({"name": "empty"})),
        ];
        let (lines, _) = run(reference, &stream, JoinKind::Inner, Direction::Contains);
        assert_eq!(names(&lines), vec![json!("big"), json!("small")]);
        assert_eq!(
            lines[0]["properties"]["matches"],
            json!([{"name": "only a"}, {"name": "a and b"}])
        );
        assert_eq!(
            lines[1]["properties"]["matches"],
            json!([{"name": "a and b"}])
        );
    }

    #[test]
    fn test_contains_polygon_in_polygon() {
        let stream = vec![feature(square(-1.0, -1.0, 20.0), json!({"name": "outer"}))];
        let (lines, _) = run(polygons(), &stream, JoinKind::Inner, Direction::Contains);
        assert_eq!(
            lines[0]["properties"]["matches"],
            json!([{"zone": "a"}, {"zone": "b"}])
        );
    }

    #[test]
    fn test_boundary_points_are_not_contained() {
        let stream = vec![feature(point(0.0, 5.0), json!({"name": "edge"}))];
        let (lines, _) = run(polygons(), &stream, JoinKind::Anti, Direction::Within);
        assert_eq!(names(&lines), vec![json!("edge")]);
    }

    #[test]
    fn test_missing_properties_get_created() {
        let stream: Vec<Feature> = vec![serde_json::from_value(json!({
            "type": "Feature", "geometry": point(1.0, 1.0), "properties": null
        }))
        .unwrap()];
        let (lines, _) = run(polygons(), &stream, JoinKind::Inner, Direction::Within);
        assert_eq!(lines[0]["properties"], json!({"matches": [{"zone": "a"}]}));
    }

    #[test]
    fn test_bad_lines_are_counted_and_skipped() {
        let reference = vec![
            feature(square(0.0, 0.0, 10.0), json!({"zone": "a"})),
            serde_json::from_value(json!({"type": "Feature", "geometry": null, "properties": {}}))
                .unwrap(),
        ];
        let input = format!(
            "{}\n{}\nnot json\n{}\n",
            json!({"type": "Point", "coordinates": [1, 1]}),
            json!({"type": "Feature", "geometry": null, "properties": {}}),
            feature(point(1.0, 1.0), json!({"name": "ok"})),
        );
        let options = JoinOptions {
            kind: JoinKind::Inner,
            direction: Direction::Within,
            field_name: "matches".to_string(),
        };
        let mut out = vec![];
        let summary = join_features(reference, &options, input.as_bytes(), &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap().lines().count(), 1);
        assert_eq!(summary.skipped_reference, 1);
        assert_eq!(summary.skipped_stream, 3);
        assert_eq!(summary.first_error, Some((1, "not a Feature".to_string())));
    }
}
