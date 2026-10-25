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

use clap::{Arg, ArgAction, Command};
use ndjson::cli;
use std::{fs::File, io::Write, process::exit};

mod area;
mod centroid;
mod common;
mod from_geojson;
mod functions;
mod intersection;
mod join_contains;
mod nearest_distance;
mod to_geojson;
mod transform;

use transform::CrsSpecification;

fn main() {
    let args = parse_args().get_matches();

    let registry = functions::registry();
    if let Some(args) = args.subcommand_matches("select") {
        exit(cli::run_select(args, &registry));
    } else if let Some(args) = args.subcommand_matches("filter") {
        exit(cli::run_filter(args, &registry));
    } else if let Some(args) = args.subcommand_matches("sort") {
        exit(cli::run_sort(args, &registry));
    } else if let Some("nearest-distance") = args.subcommand_name() {
        let args = args
            .subcommand_matches("nearest-distance")
            .expect("subcommand was correctly tested for");
        let filename = args
            .get_one::<String>("reference")
            .map(String::as_str)
            .expect("reference is required");

        let reference_file = match File::open(filename) {
            Ok(r) => r,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };
        if let Err(err) = nearest_distance::nearest_distance(reference_file) {
            writeln!(
                ::std::io::stderr(),
                "Error computing nearest distance {:?}",
                err
            )
            .expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("join-contains") {
        exit(run_join_contains(args));
    } else if let Some("centroid") = args.subcommand_name() {
        centroid::compute_centroid();
    } else if let Some("intersection") = args.subcommand_name() {
        let args = args
            .subcommand_matches("intersection")
            .expect("subcommand was correctly tested for");
        let filename = args
            .get_one::<String>("reference")
            .map(String::as_str)
            .expect("reference is required");

        let geo_type = args
            .get_one::<String>("geo-type")
            .map(String::as_str)
            .expect("geometry type is required");

        let reference_file = match File::open(filename) {
            Ok(r) => r,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };
        if let Err(err) = intersection::intersection(reference_file, geo_type) {
            writeln!(
                ::std::io::stderr(),
                "Error computing intersection {:?}",
                err
            )
            .expect("Unable to write to stderr");
        }
    } else if let Some("area") = args.subcommand_name() {
        let args = args
            .subcommand_matches("area")
            .expect("subcommand was tested for");

        let field_name = args
            .get_one::<String>("field-name")
            .map(String::as_str)
            .expect("field-name is required")
            .into();

        let bbox = args.get_flag("bbox");
        let planar = args.get_flag("planar");

        if let Err(e) = area::NdjsonSpatialArea::default().area(field_name, bbox, planar) {
            writeln!(std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    } else if let Some("to-geojson") = args.subcommand_name() {
        if let Err(e) = to_geojson::to_geojson() {
            writeln!(std::io::stderr(), "{:?}", e).expect("Could not write to stderr");
        }
    } else if let Some("from-geojson") = args.subcommand_name() {
        if let Err(e) = from_geojson::split() {
            writeln!(std::io::stderr(), "{:?}", e).expect("Could not write to stderr");
        }
    } else if let Some("transform") = args.subcommand_name() {
        let args = args
            .subcommand_matches("transform")
            .expect("The correct command is tranform");

        let to = to_crs_specification(
            args.get_one::<String>("to-epsg").map(String::as_str),
            args.get_one::<String>("to-proj4").map(String::as_str),
        );

        let from = to_crs_specification(
            args.get_one::<String>("from-epsg").map(String::as_str),
            args.get_one::<String>("from-proj4").map(String::as_str),
        );

        if let Err(e) = transform::transform(
            std::io::BufReader::new(std::io::stdin()),
            from,
            to,
            &mut std::io::stdout(),
        ) {
            writeln!(std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    }
}

fn run_join_contains(args: &clap::ArgMatches) -> i32 {
    let filename = args
        .get_one::<String>("reference")
        .expect("reference is required");
    let reference_file = match File::open(filename) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("join-contains: error opening reference file: {}", e);
            return 1;
        }
    };
    let kind = if args.get_flag("left") {
        join_contains::JoinKind::Left
    } else if args.get_flag("semi") {
        join_contains::JoinKind::Semi
    } else if args.get_flag("anti") {
        join_contains::JoinKind::Anti
    } else {
        join_contains::JoinKind::Inner
    };
    let direction = if args.get_flag("within") {
        join_contains::Direction::Within
    } else {
        join_contains::Direction::Contains
    };
    let options = join_contains::JoinOptions {
        kind,
        direction,
        field_name: args
            .get_one::<String>("field-name")
            .expect("field-name has a default")
            .clone(),
    };
    let input = std::io::BufReader::with_capacity(1 << 20, std::io::stdin().lock());
    let mut output = std::io::BufWriter::with_capacity(1 << 20, std::io::stdout().lock());
    match join_contains::join_contains(reference_file, &options, input, &mut output) {
        Ok(summary) => {
            if summary.skipped_reference > 0 {
                eprintln!(
                    "join-contains: skipped {} reference feature{} without a usable geometry",
                    summary.skipped_reference,
                    if summary.skipped_reference == 1 {
                        ""
                    } else {
                        "s"
                    }
                );
            }
            if let Some((line, err)) = summary.first_error {
                eprintln!(
                    "join-contains: skipped {} line{} (first at line {}: {})",
                    summary.skipped_stream,
                    if summary.skipped_stream == 1 { "" } else { "s" },
                    line,
                    err
                );
            }
            0
        }
        Err(ndjson_common::error::NdJsonSpatialError::Error(e)) => {
            eprintln!("join-contains: {}", e);
            1
        }
    }
}

fn to_crs_specification(epsg: Option<&str>, proj4: Option<&str>) -> CrsSpecification {
    match (epsg, proj4) {
        (Some(epsg), None) => match epsg.parse() {
            Ok(epsg) => CrsSpecification::Epsg(epsg),
            Err(e) => {
                writeln!(std::io::stderr(), "{}", e).expect("Unable to write to stderr");
                exit(1);
            }
        },
        (None, Some(proj4)) => CrsSpecification::Proj(proj4.to_string()),
        _ => panic!("Unreachable"),
    }
}

const SPATIAL_FUNCTIONS: &str = "Geometry functions (g is a GeoJSON geometry or Feature):\n  \
    area length (m², m from lon/lat) planar_area planar_length\n  \
    centroid point_on_surface bbox x y\n  \
    geom_type num_points is_valid is_empty convex_hull make_valid to_wkt from_wkt\n  \
    simplify(g, tol) buffer(g, d) transform(g, from, to)\n  \
    distance intersects contains within (g, wkt or geometry)\n  \
    geohash(g, precision) quadkey(g, zoom) h3(g, res)\n  \
    aggregates: extent centroid_agg union_agg";

fn parse_args() -> Command {
    Command::new("ndjson-spatial")
        .about("computes spatial metrics on new-line delimited json")
        .subcommand(cli::select_command().after_help(SPATIAL_FUNCTIONS))
        .subcommand(cli::filter_command().after_help(SPATIAL_FUNCTIONS))
        .subcommand(cli::sort_command())
        .subcommand(
            Command::new("nearest-distance")
                .about("compute the distance to the nearest point in 'reference' json file")
                .arg(
                    Arg::new("reference")
                        .short('r')
                        .long("ref")
                        .required(true)
                        .num_args(1)
                        .help("The geojson file to search within for nearest point's distance"),
                ),
        )
        .subcommand(
            Command::new("join-contains")
                .about("joins geojson features with the reference features they contain, or with --within the ones that contain them")
                .arg(
                    Arg::new("reference")
                        .short('r')
                        .long("ref")
                        .required(true)
                        .num_args(1)
                        .help("The geojson FeatureCollection to join on; held in memory in an R-tree"),
                )
                .arg(
                    Arg::new("field-name")
                        .long("field-name")
                        .visible_alias("into")
                        .num_args(1)
                        .default_value("matches")
                        .conflicts_with_all(["semi", "anti"])
                        .help("The property to store the matching reference features' properties in, as an array"),
                )
                .arg(
                    Arg::new("within")
                        .long("within")
                        .action(ArgAction::SetTrue)
                        .help("Join each feature with the reference features that contain it (point-in-polygon), instead of the ones it contains"),
                )
                .arg(
                    Arg::new("left")
                        .long("left")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["semi", "anti"])
                        .help("Also write features without a match, with an empty array"),
                )
                .arg(
                    Arg::new("semi")
                        .long("semi")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("anti")
                        .help("Write features with a match, unchanged"),
                )
                .arg(
                    Arg::new("anti")
                        .long("anti")
                        .action(ArgAction::SetTrue)
                        .help("Write features without a match, unchanged"),
                ),
        )
        .subcommand(
            Command::new("centroid")
                .about("compute the centroid of multipolygon or polygon ndjson stream"),
        )
        .subcommand(Command::new("from-geojson").about("Convert geojson to ndjson"))
        .subcommand(Command::new("to-geojson").about("Convert ndjson to geojson"))
        .subcommand(
            Command::new("intersection")
                .about("compute the intersection with polygons in a reference file")
                .arg(
                    Arg::new("reference")
                        .short('r')
                        .long("ref")
                        .required(true)
                        .num_args(1)
                        .help("The geojson file to search within for Geometry types"),
                )
                .arg(
                    Arg::new("geo-type")
                        .short('g')
                        .long("geo-type")
                        .required(true)
                        .num_args(1)
                        .help("The OGC Geometry type in the reference file"),
                ),
        )
        .subcommand(
            Command::new("area")
                .about("compute the area of the shape represented in ndjson, in square meters")
                .arg(
                    Arg::new("field-name")
                        .short('f')
                        .long("field-name")
                        .required(true)
                        .num_args(1)
                        .help("what to name the area field"),
                )
                .arg(
                    Arg::new("bbox")
                        .short('b')
                        .long("bbox")
                        .action(ArgAction::SetTrue)
                        .help("Compute the bounding box if it does not exist"),
                )
                .arg(
                    Arg::new("planar")
                        .long("planar")
                        .action(ArgAction::SetTrue)
                        .help("Planar area in the units of projected coordinates, not lon/lat"),
                ),
        )
        .subcommand(
            Command::new("transform")
                .about("transform the coordinate reference system")
                .arg(
                    Arg::new("from-epsg")
                        .long("from-epsg")
                        .required_unless_present("from-proj4")
                        .conflicts_with("from-proj4")
                        .num_args(1)
                        .help("The epsg that the geometry is in"),
                )
                .arg(
                    Arg::new("to-epsg")
                        .long("to-epsg")
                        .required_unless_present("to-proj4")
                        .conflicts_with("to-proj4")
                        .num_args(1)
                        .help("The epsg the the geometry is tranformed to"),
                )
                .arg(
                    Arg::new("from-proj4")
                        .long("from-proj4")
                        .required_unless_present("from-epsg")
                        .conflicts_with("from-epsg")
                        .num_args(1)
                        .help("The proj string the geometry is in"),
                )
                .arg(
                    Arg::new("to-proj4")
                        .long("to-proj4")
                        .required_unless_present("to-epsg")
                        .conflicts_with("to-epsg")
                        .num_args(1)
                        .help("The proj string the geometry is transformed to"),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn verify_cli() {
        parse_args().debug_assert();
    }

    #[test]
    fn transform_requires_one_of_each_crs() {
        let cmd = parse_args;
        assert!(cmd()
            .try_get_matches_from(["ndjson-spatial", "transform", "--from-epsg", "4326"])
            .is_err());
        assert!(cmd()
            .try_get_matches_from([
                "ndjson-spatial",
                "transform",
                "--from-epsg",
                "4326",
                "--from-proj4",
                "+proj=longlat",
                "--to-epsg",
                "3857",
            ])
            .is_err());
        let m = cmd()
            .try_get_matches_from([
                "ndjson-spatial",
                "transform",
                "--from-epsg",
                "4326",
                "--to-proj4",
                "+proj=merc",
            ])
            .expect("valid transform args");
        let m = m.subcommand_matches("transform").expect("transform");
        assert_eq!(
            m.get_one::<String>("from-epsg").map(String::as_str),
            Some("4326")
        );
        assert_eq!(
            m.get_one::<String>("to-proj4").map(String::as_str),
            Some("+proj=merc")
        );
    }

    #[test]
    fn area_bbox_is_a_flag() {
        let m = parse_args()
            .try_get_matches_from(["ndjson-spatial", "area", "-f", "a", "-b"])
            .expect("valid area args");
        let m = m.subcommand_matches("area").expect("area");
        assert!(m.get_flag("bbox"));
        assert_eq!(
            m.get_one::<String>("field-name").map(String::as_str),
            Some("a")
        );
    }

    #[test]
    fn join_contains_kinds_conflict() {
        let parse = |args: &[&str]| {
            parse_args().try_get_matches_from(
                ["ndjson-spatial", "join-contains", "-r", "ref.geojson"]
                    .iter()
                    .chain(args),
            )
        };
        let m = parse(&["--within", "--left"]).expect("valid join-contains args");
        let m = m
            .subcommand_matches("join-contains")
            .expect("join-contains");
        assert!(m.get_flag("within") && m.get_flag("left"));
        assert_eq!(
            m.get_one::<String>("field-name").map(String::as_str),
            Some("matches")
        );
        assert!(parse(&["--into", "zones"]).is_ok());
        assert!(parse(&["--semi", "--anti"]).is_err());
        assert!(parse(&["--left", "--anti"]).is_err());
        assert!(parse(&["--semi", "--into", "zones"]).is_err());
    }
}
