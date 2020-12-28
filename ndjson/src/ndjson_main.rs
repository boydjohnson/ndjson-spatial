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

#![feature(move_ref_pattern)]

use clap::{App, Arg, ArgMatches, SubCommand};
use ndjson_common::json_selector_parser::{parse_json_selector, Selector};
use std::{
    fs::File,
    io::{stdin, stdout, BufReader, BufWriter, Write},
    process::exit,
};

mod filter;
mod from_json;
mod join;
mod pick_field;
mod to_json;

fn main() {
    let args = parse_args();

    if let Some("filter") = args.subcommand_name() {
        let args = args
            .subcommand_matches("filter")
            .expect("subcommand was correctly tested for");
        let expression = args
            .value_of("expression")
            .expect("expression is required")
            .to_string();

        if let Err(err) =
            filter::ndjson_filter(expression, &mut stdin().lock(), &mut stdout().lock())
        {
            writeln!(::std::io::stderr(), "{:?}", err).expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("pick-field") {
        let expression = match parse_json_selector(
            args.value_of("expression")
                .expect("expression is required")
                .into(),
        ) {
            Ok(s) => s.1,
            Err(e) => {
                println!("Error parsing expression: {}", e);
                exit(1)
            }
        };

        if let Err(e) = pick_field::pick_field(
            expression,
            &mut BufReader::with_capacity(1_000_000, &mut stdin().lock()),
            BufWriter::with_capacity(1_000_000, stdout().lock()),
        ) {
            writeln!(::std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    } else if let Some("join") = args.subcommand_name() {
        let args = args
            .subcommand_matches("join")
            .expect("subcommand was correctly tested for");
        let filename = args.value_of("reference").expect("reference is required");
        let mut reference_file = match File::open(filename) {
            Ok(r) => BufReader::new(r),
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };

        let ref_fields: Vec<Vec<Selector>> = match args
            .values_of("reference-fields")
            .expect("reference-fields is required")
            .map(|s| parse_json_selector(s.into()).map(|(_, result)| result))
            .collect()
        {
            Ok(v) => v,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error parsing reference-fields: {}", e)
                    .expect("Unable to write to stderr");
                exit(1)
            }
        };
        let stream_fields: Vec<Vec<Selector>> = match args
            .values_of("stream-fields")
            .expect("stream-fields is required")
            .map(|s| parse_json_selector(s.into()).map(|(_, result)| result))
            .collect()
        {
            Ok(v) => v,
            Err(e) => {
                writeln!(::std::io::stderr(), "Error parsing stream-fields: {}", e)
                    .expect("Unable to write to stderr");
                exit(1)
            }
        };

        if let Err(err) = join::join(
            &mut reference_file,
            ref_fields,
            stream_fields,
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        ) {
            writeln!(::std::io::stderr(), "{:?}", err).expect("Error writing to stderr");
        }
    } else if let Some("from-json") = args.subcommand_name() {
        let args = args
            .subcommand_matches("from-json")
            .expect("subcommand was correctly tested for");
        let expression = args.value_of("expression").expect("expression is required");

        if let Err(e) = from_json::from_json(expression) {
            writeln!(std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    }
}

fn parse_args<'a>() -> ArgMatches<'a> {
    App::new("ndjson")
        .about("Tool for working with ndjson")
        .subcommand(
            SubCommand::with_name("pick-field")
                .about("picks a field from all of the ndjson objects")
                .arg(
                    Arg::with_name("expression")
                        .required(true)
                        .help("the expression that yields a field"),
                ),
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
                        .help("The fields in the reference file to join on"),
                )
                .arg(
                    Arg::with_name("stream-fields")
                        .long("stream-fields")
                        .required(true)
                        .takes_value(true)
                        .help("The fields in the ndjson stream to join on"),
                ),
        )
        .subcommand(
            SubCommand::with_name("filter")
                .about("returns only json that matches filter expression")
                .arg(
                    Arg::with_name("expression")
                        .required(true)
                        .help("The json selector filter expression"),
                ),
        )
        .subcommand(
            SubCommand::with_name("from-json")
                .about("Converts json to ndjson")
                .arg(
                    Arg::with_name("expression")
                        .required(true)
                        .help("selector expression that contains the collection"),
                ),
        )
        .get_matches()
}
