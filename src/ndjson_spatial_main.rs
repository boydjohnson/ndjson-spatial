extern crate clap;

use clap::{App, Arg, ArgMatches, SubCommand};
use std::fs::File;
use std::io::Write;
use std::process::exit;

mod area;
mod centroid;
mod error;
mod intersection;
mod join_contains;
mod ndjson;
mod nearest_distance;

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
        let reference_file = match File::open(filename) {
            Ok(r) => r,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };
        if let Err(err) = intersection::intersection(reference_file) {
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

        if let Err(e) = area::NdjsonSpatialArea::default().area(field_name) {
            writeln!(std::io::stderr(), "{:?}", e);
        }
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
            SubCommand::with_name("intersection")
                .about("compute the intersection with polygons in a reference file")
                .arg(
                    Arg::with_name("reference")
                        .short("r")
                        .long("ref")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The geojson file to search within for polygons"),
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
        )
        .get_matches()
}
