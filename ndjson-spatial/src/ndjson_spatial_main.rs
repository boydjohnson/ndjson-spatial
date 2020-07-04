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

use clap::{App, Arg, ArgMatches, SubCommand};
use std::{fs::File, io::Write, process::exit};

mod area;
mod centroid;
mod common;
mod from_geojson;
mod intersection;
mod nearest_distance;
mod to_geojson;
mod transform;

use transform::CrsSpecification;

fn main() {
    let args = parse_args();

    if let Some("nearest-distance") = args.subcommand_name() {
        let args = args
            .subcommand_matches("nearest-distance")
            .expect("subcommand was correctly tested for");
        let filename = args.value_of("reference").expect("reference is required");

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
    } else if let Some("centroid") = args.subcommand_name() {
        centroid::compute_centroid();
    } else if let Some("intersection") = args.subcommand_name() {
        let args = args
            .subcommand_matches("intersection")
            .expect("subcommand was correctly tested for");
        let filename = args.value_of("reference").expect("reference is required");

        let geo_type = args
            .value_of("geo-type")
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
            .value_of("field-name")
            .expect("field-name is required")
            .into();

        let bbox = args.is_present("bbox");

        if let Err(e) = area::NdjsonSpatialArea::default().area(field_name, bbox) {
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

        let to = to_crs_specification(args.value_of("to-epsg"), args.value_of("to-proj4"));

        let from = to_crs_specification(args.value_of("from-epsg"), args.value_of("from-proj4"));

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

fn parse_args<'a>() -> ArgMatches<'a> {
    App::new("ndjson-spatial")
        .about("computes spatial metrics on new-line delimited json")
        .subcommand(
            SubCommand::with_name("nearest-distance")
                .about("compute the distance to the nearest point in 'reference' json file")
                .arg(
                    Arg::with_name("reference")
                        .short("r")
                        .long("ref")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The geojson file to search within for nearest point's distance"),
                )
        )
        .subcommand(
            SubCommand::with_name("join-contains")
                .about("joins ndjson objects with contained points, lines, polygons, or multipolygons in a reference file")
                .arg(
                    Arg::with_name("reference")
                        .short("r")
                        .long("ref")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The geojson file to join on.")
                )
                .arg(
                    Arg::with_name("field-name")
                        .long("field-name")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The name of the field to store the contained objects")
                )

        )
        .subcommand(
            SubCommand::with_name("centroid")
                .about("compute the centroid of multipolygon or polygon ndjson stream"),
        )
        .subcommand(
            SubCommand::with_name("from-geojson")
                .about("Convert geojson to ndjson")
        )
        .subcommand(
            SubCommand::with_name("to-geojson")
                .about("Convert ndjson to geojson")
        )
        .subcommand(
            SubCommand::with_name("intersection")
                .about("compute the intersection with polygons in a reference file")
                .arg(
                    Arg::with_name("reference")
                        .short("r")
                        .long("ref")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The geojson file to search within for Geometry types"),
                ).arg(
                    Arg::with_name("geo-type")
                        .short("g")
                        .long("geo-type")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The OGC Geometry type in the reference file")
                )
        )
        .subcommand(
            SubCommand::with_name("area")
                .about("compute the area of the shape represented in ndjson")
                .arg(
                    Arg::with_name("field-name")
                        .short("f")
                        .long("field-name")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("what to name the area field")
                )
                .arg(
                    Arg::with_name("bbox")
                        .short("b")
                        .long("bbox")
                        .required(false)
                        .takes_value(false)
                        .help("Compute the bounding box if it does not exist")
                )
        )
        .subcommand(
            SubCommand::with_name("transform")
                .about("transform the coordinate reference system")
                .arg(
                    Arg::with_name("from-epsg")
                        .long("from-epsg")
                        .required_unless("from-proj4")
                        .conflicts_with("from-proj4")
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The epsg that the geometry is in")
                )
                .arg(
                    Arg::with_name("to-epsg")
                        .long("to-epsg")
                        .required_unless("to-proj4")
                        .conflicts_with("to-proj4")
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The epsg the the geometry is tranformed to")
                )
                .arg(
                    Arg::with_name("from-proj4")
                        .long("from-proj4")
                        .required_unless("from-epsg")
                        .conflicts_with("from-epsg")
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The proj string the geometry is in")
                )
                .arg(
                    Arg::with_name("to-proj4")
                        .long("to-proj4")
                        .required_unless("to-epsg")
                        .conflicts_with("to-epsg")
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The proj string the geometry is transformed to")
                )
        )
        .get_matches()
}
