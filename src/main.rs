extern crate clap;

use clap::{App, Arg, ArgMatches, SubCommand};
use std::fs::File;
use std::io::Write;
use std::process::exit;

mod area;
mod centroid;
mod error;
mod intersection;
mod join;
mod ndjson;
mod nearest_distance;
mod split;

fn main() {
    let args = parse_args();

    if let Some("nearest-distance") = args.subcommand_name() {
        let args = args
            .subcommand_matches("nearest-distance")
            .expect("subcommand was correctly tested for");
        let filename = args.value_of("reference").expect("reference is required");

        let property = args
            .value_of("property")
            .expect("property is required")
            .to_string();

        let reference_file = match File::open(filename) {
            Ok(r) => r,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };
        if let Err(err) = nearest_distance::nearest_distance(reference_file, property) {
            writeln!(
                ::std::io::stderr(),
                "Error computing nearest distance {:?}",
                err
            )
            .expect("Unable to write to stderr");
        }
    } else if let Some("centroid") = args.subcommand_name() {
        centroid::compute_centroid();
    } else if let Some("join") = args.subcommand_name() {
        let args = args
            .subcommand_matches("join")
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

        let ref_fields: Vec<String> = args
            .values_of("reference-fields")
            .expect("reference-fields is required")
            .map(|s| s.into())
            .collect();
        let stream_fields: Vec<String> = args
            .values_of("stream-fields")
            .expect("stream-fields is required")
            .map(|s| s.into())
            .collect();

        join::join(reference_file, ref_fields, stream_fields).unwrap();
    } else if let Some("split") = args.subcommand_name() {
        if let Err(err) = split::split() {
            writeln!(::std::io::stderr(), "Error in split: {:?}", err)
                .expect("Unable to write to stderr");
        }
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

        area::area(field_name).unwrap();
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
                .arg(
                    Arg::with_name("property")
                        .short("p")
                        .long("prop")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("A property on the geojson value to include with the distance as an identifier")
                ),
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
            SubCommand::with_name("split")
                .about("splits geojson feature collection into ndjson stream"),
        )
        .subcommand(
            SubCommand::with_name("join")
                .about("joins json file to ndjson stream")
                .arg(
                    Arg::with_name("reference")
                        .short("r")
                        .long("ref")
                        .required(true)
                        .takes_value(true)
                        .number_of_values(1)
                        .help("The ndjson file to join on"),
                )
                .arg(
                    Arg::with_name("reference-fields")
                        .long("ref-fields")
                        .required(true)
                        .takes_value(true)
                        .help("The fields in the reference file to join on")
                )
                .arg(
                    Arg::with_name("stream-fields")
                        .long("stream-fields")
                        .required(true)
                        .takes_value(true)
                        .help("The fields in the ndjson stream to join on")
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
