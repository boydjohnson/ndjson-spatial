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

use clap::{command, Arg, ArgAction, ArgMatches, Command};
use ndjson::{
    aggregate::{aggregate, Aggregation},
    filter, from_json, join, pick_field,
};
use ndjson_common::{
    error::NdJsonSpatialError,
    json_selector_parser::{parse_json_selector, Selector},
};
use std::{
    fs::File,
    io::{stdin, stdout, BufReader, BufWriter, Write},
    process::exit,
};

fn main() {
    let args = parse_args();

    if let Some("filter") = args.subcommand_name() {
        let args = args
            .subcommand_matches("filter")
            .expect("subcommand was correctly tested for");
        let expression = args
            .get_one::<String>("expression")
            .map(String::as_str)
            .expect("expression is required")
            .to_string();

        if let Err(err) =
            filter::ndjson_filter(expression, &mut stdin().lock(), &mut stdout().lock())
        {
            writeln!(::std::io::stderr(), "{:?}", err).expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("pick-field") {
        let expression = match parse_json_selector(
            args.get_one::<String>("expression")
                .map(String::as_str)
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
        let filename = args
            .get_one::<String>("reference")
            .map(String::as_str)
            .expect("reference is required");
        let mut reference_file = match File::open(filename) {
            Ok(r) => BufReader::new(r),
            Err(e) => {
                writeln!(::std::io::stderr(), "Error opening reference file: {}", e)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        };

        let ref_fields: Vec<Vec<Selector>> = match args
            .get_many::<String>("reference-fields")
            .map(|v| v.map(String::as_str))
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
            .get_many::<String>("stream-fields")
            .map(|v| v.map(String::as_str))
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
        let expression = args
            .get_one::<String>("expression")
            .map(String::as_str)
            .expect("expression is required");

        if let Err(e) = from_json::from_json(expression) {
            writeln!(std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("from-csv") {
        let delimiter = args
            .get_one::<String>("delimiter")
            .map(String::as_str)
            .and_then(|s| s.chars().next())
            .map(|d| d as u8)
            .unwrap_or(b',');

        if let Err(e) = ndjson::from_csv::from_csv(delimiter) {
            writeln!(std::io::stderr(), "{:?}", e).expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("agg") {
        let aggregator_selector = args
            .get_many::<String>("aggregator")
            .map(|v| v.map(String::as_str))
            .map(|mut v| {
                (
                    v.next().unwrap(),
                    parse_json_selector(v.next().unwrap().into()).map(|v| v.1),
                )
            })
            .map(|(first, second)| match (first, second) {
                ("count", Ok(selector)) => Aggregation::Count(selector),
                ("min", Ok(selector)) => Aggregation::Min(selector),
                ("max", Ok(selector)) => Aggregation::Max(selector),
                ("sum", Ok(selector)) => Aggregation::Sum(selector),
                (&_, Err(e)) => {
                    writeln!(
                        std::io::stderr(),
                        "Error parsing aggregation selector: {}",
                        e
                    )
                    .expect("Unable to write to stderr");
                    exit(1);
                }
                (a, Ok(_)) => {
                    writeln!(
                        std::io::stderr(),
                        "--agg must be one of 'count', 'sum', 'min', 'max', not: {}",
                        a
                    )
                    .expect("Unable to write to stderr");
                    exit(1);
                }
            })
            .expect("aggregator is required");

        let group_by_selector = match parse_json_selector(
            args.get_one::<String>("group-by")
                .map(String::as_str)
                .expect("group-by is required")
                .into(),
        )
        .map(|f| f.1)
        {
            Ok(s) => s,
            Err(e) => {
                writeln!(std::io::stderr(), "Error parsing group-by selector: {}", e)
                    .expect("Unable to write to stderr");
                exit(1)
            }
        };

        if let Err(e) = aggregate(
            aggregator_selector,
            group_by_selector,
            &mut BufReader::with_capacity(500_000, &mut stdin().lock()),
            BufWriter::with_capacity(1_000_000, stdout().lock()),
        ) {
            writeln!(std::io::stderr(), "Error: {:?}", e).expect("Unable to write to stderr");
        }
    } else if let Some(args) = args.subcommand_matches("sort") {
        if let Some(fields) = args
            .get_many::<String>("fields")
            .map(|v| v.map(String::as_str))
        {
            let selectors = match fields
                .map(|f| {
                    if let Some((selector, sort_order)) = f.split_once(':') {
                        match parse_json_selector(selector.into()) {
                            Ok((_, selectors)) => Ok((selectors, parse_sort_order(sort_order))),
                            Err(e) => Err(NdJsonSpatialError::Error(format!(
                                "Failed to parse selector: {}: {}",
                                selector, e
                            ))),
                        }
                    } else {
                        match parse_json_selector(f.into()) {
                            Ok((_, selectors)) => Ok((selectors, true)),
                            Err(e) => Err(NdJsonSpatialError::Error(format!(
                                "Failed to parse selector: {}: {}",
                                f, e
                            ))),
                        }
                    }
                })
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(selectors) => selectors,
                Err(e) => {
                    eprintln!("{:?}", e);
                    std::process::exit(1);
                }
            };

            let input = BufReader::with_capacity(2000, std::io::stdin());

            if let Err(e) = ndjson::sort::sort(input, &mut std::io::stdout(), selectors) {
                eprintln!("{:?}", e);
                std::process::exit(1);
            }
        }
    }
}

fn parse_sort_order(s: &str) -> bool {
    match s {
        "asc" => true,
        "a" => true,
        "desc" => false,
        "d" => false,
        a => {
            eprintln!("expected 'asc', 'a', 'desc' or 'd' found {}", a);
            std::process::exit(1);
        }
    }
}

fn parse_args() -> ArgMatches {
    command!()
        .subcommand(
            Command::new("pick-field")
                .about("picks a field from all of the ndjson objects")
                .arg(
                    Arg::new("expression")
                        .required(true)
                        .help("the expression that yields a field"),
                ),
        )
        .subcommand(
            Command::new("join")
                .about("joins json file to ndjson stream")
                .arg(
                    Arg::new("reference")
                        .short('r')
                        .long("ref")
                        .required(true)
                        .num_args(1)
                        .help("The ndjson file to join on"),
                )
                .arg(
                    Arg::new("reference-fields")
                        .long("ref-fields")
                        .required(true)
                        .num_args(1..)
                        .help("The fields in the reference file to join on, matched as a composite key"),
                )
                .arg(
                    Arg::new("stream-fields")
                        .long("stream-fields")
                        .required(true)
                        .num_args(1..)
                        .help("The fields in the ndjson stream to join on, matched as a composite key"),
                ),
        )
        .subcommand(
            Command::new("filter")
                .about("returns only json that matches filter expression")
                .arg(
                    Arg::new("expression")
                        .required(true)
                        .help("The json selector filter expression"),
                ),
        )
        .subcommand(
            Command::new("from-csv")
                .about("Converts csv to ndjson")
                .arg(
                    Arg::new("delimiter")
                        .num_args(1)
                        .help("The delimiter of the csv"),
                ),
        )
        .subcommand(
            Command::new("from-json")
                .about("Converts json to ndjson")
                .arg(
                    Arg::new("expression")
                        .required(true)
                        .help("selector expression that contains the collection"),
                ),
        )
        .subcommand(
            Command::new("agg")
                .about("Aggregatation commands on a grouped-by key")
                .arg(
                    Arg::new("group-by")
                        .short('g')
                        .long("group-by")
                        .help("Selector for field to group by")
                        .num_args(1)
                        .value_name("selector")
                        .required(true),
                )
                .arg(
                    Arg::new("aggregator")
                        .short('a')
                        .long("agg")
                        .required(true)
                        .num_args(2)
                        .value_names(["aggregator", "selector"])
                        .help("aggregation function along with selector. e.g. -a sum d.salary"),
                ),
        )
        .subcommand(
            Command::new("sort")
                .about("Sort by ndjson fields")
                .arg(
                    Arg::new("fields")
                        .short('f')
                        .long("fields")
                        .help("Selector for fields to sort by. e.g. -f d.property_value or -f d.property_value:d")
                        .num_args(1..)
                        .action(ArgAction::Append)
                        .value_name("selector")
                        .required(true),
                ),
        )
        .get_matches()
}
