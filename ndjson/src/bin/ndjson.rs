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
    aggregate::{aggregate, AggOptions},
    filter, from_json,
    group::{AggregateSummary, Grouping},
    head, join, pick_field, select, tail,
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

        match filter::ndjson_filter(expression, &mut stdin().lock(), &mut stdout().lock()) {
            Ok(filter::FilterSummary {
                errors,
                first_error: Some((record, err)),
            }) => {
                writeln!(
                    ::std::io::stderr(),
                    "filter: skipped {} record{} that failed to evaluate (first at record {}: {})",
                    errors,
                    if errors == 1 { "" } else { "s" },
                    record,
                    err
                )
                .expect("Unable to write to stderr");
            }
            Ok(_) => {}
            Err(NdJsonSpatialError::Error(err)) => {
                writeln!(::std::io::stderr(), "filter: {}", err)
                    .expect("Unable to write to stderr");
                exit(1);
            }
        }
    } else if let Some(args) = args.subcommand_matches("select") {
        let list = args.get_one::<String>("list").expect("list is required");
        let mode = if args.get_flag("raw") {
            select::OutputMode::Raw
        } else if args.get_flag("tsv") {
            select::OutputMode::Tsv
        } else {
            select::OutputMode::Object
        };
        let list = match select::parse_select_list(list, mode) {
            Ok(list) => list,
            Err(err) => {
                eprintln!("select: {}", err);
                exit(1);
            }
        };
        for warning in &list.warnings {
            eprintln!("select: {}", warning);
        }
        let by = match args.get_one::<String>("by").map(|by| select::parse_by(by)) {
            None => vec![],
            Some(Ok(by)) => by,
            Some(Err(err)) => {
                eprintln!("select: {}", err);
                exit(1);
            }
        };
        let strategy = if args.get_flag("hash") {
            select::GroupStrategy::Hash {
                max_groups: *args
                    .get_one::<usize>("max-groups")
                    .expect("max-groups has a default"),
            }
        } else if args.get_flag("grouped") {
            select::GroupStrategy::Grouped
        } else {
            select::GroupStrategy::Sorted
        };
        let options = select::GroupOptions {
            by,
            strategy,
            allow_unbounded: args.get_flag("allow-unbounded"),
        };
        let mut output = BufWriter::with_capacity(1_000_000, stdout().lock());
        match select::ndjson_select_grouped(&list, mode, &options, &mut stdin().lock(), &mut output)
        {
            Ok(summary) => {
                for item in summary.errors {
                    eprintln!(
                        "select: {} error{} in '{}', {} (first at record {}: {})",
                        item.count,
                        if item.count == 1 { "" } else { "s" },
                        item.item,
                        if summary.aggregate {
                            "skipped"
                        } else {
                            "written as null"
                        },
                        item.first.0,
                        item.first.1
                    );
                }
            }
            Err(NdJsonSpatialError::Error(err)) => {
                eprintln!("select: {}", err);
                exit(1);
            }
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
        let strings = |name: &str| -> Vec<String> {
            args.get_many::<String>(name)
                .map(|v| v.cloned().collect())
                .unwrap_or_default()
        };
        let aggregates = strings("aggregator")
            .chunks(2)
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect();
        let grouping = if args.get_flag("grouped") {
            Grouping::Sorted { check_order: false }
        } else if args.get_flag("sorted") {
            Grouping::Sorted { check_order: true }
        } else {
            Grouping::Hash {
                max_groups: *args
                    .get_one::<usize>("max-groups")
                    .expect("max-groups has a default"),
            }
        };
        let options = AggOptions {
            group_by: strings("group-by"),
            aggregates,
            grouping,
            allow_unbounded: args.get_flag("allow-unbounded"),
        };

        match aggregate(
            &options,
            &mut BufReader::with_capacity(500_000, &mut stdin().lock()),
            BufWriter::with_capacity(1_000_000, stdout().lock()),
        ) {
            Ok(AggregateSummary {
                errors,
                first_error: Some((record, field, err)),
                ..
            }) => {
                writeln!(
                    ::std::io::stderr(),
                    "agg: skipped {} value{} that failed to evaluate (first at record {}, {}: {})",
                    errors,
                    if errors == 1 { "" } else { "s" },
                    record,
                    field,
                    err
                )
                .expect("Unable to write to stderr");
            }
            Ok(_) => {}
            Err(NdJsonSpatialError::Error(err)) => {
                writeln!(::std::io::stderr(), "agg: {}", err).expect("Unable to write to stderr");
                exit(1);
            }
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
    } else if let Some(args) = args.subcommand_matches("head") {
        let n = *args.get_one::<usize>("lines").expect("lines has a default");
        let mut output = BufWriter::new(stdout().lock());
        if let Err(e) = head::head(n, &mut stdin().lock(), &mut output) {
            eprintln!("{:?}", e);
            exit(1);
        }
    } else if let Some(args) = args.subcommand_matches("tail") {
        let n = *args.get_one::<usize>("lines").expect("lines has a default");
        let mut output = BufWriter::new(stdout().lock());
        if let Err(e) = tail::tail(n, &mut stdin().lock(), &mut output) {
            eprintln!("{:?}", e);
            exit(1);
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
            Command::new("select")
                .about("Writes a list of expressions for each record, or aggregates per group")
                .arg(
                    Arg::new("list")
                        .required(true)
                        .value_name("list")
                        .help("Comma-separated expressions, each optionally 'as <name>'; '*' is the whole record, '* except (d.a, d.b)' drops fields. e.g. 'd.name, d.price * d.qty as total'"),
                )
                .arg(
                    Arg::new("raw")
                        .short('r')
                        .long("raw")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("tsv")
                        .help("Write bare JSON values: one item as is, several as an array"),
                )
                .arg(
                    Arg::new("tsv")
                        .long("tsv")
                        .action(ArgAction::SetTrue)
                        .help("Write tab-separated values: strings unquoted, null empty"),
                )
                .arg(
                    Arg::new("by")
                        .long("by")
                        .value_name("keys")
                        .help("Aggregate per group of these comma-separated key expressions, e.g. --by 'd.state, d.county'. The input must be sorted by them unless --hash or --grouped is given"),
                )
                .arg(
                    Arg::new("hash")
                        .long("hash")
                        .action(ArgAction::SetTrue)
                        .requires("by")
                        .conflicts_with("grouped")
                        .help("Group unsorted input in memory, one set of aggregates per key (up to --max-groups), written in key order"),
                )
                .arg(
                    Arg::new("grouped")
                        .long("grouped")
                        .action(ArgAction::SetTrue)
                        .requires("by")
                        .help("Input has equal keys next to each other, in any order: group in constant memory without checking the order"),
                )
                .arg(
                    Arg::new("max-groups")
                        .long("max-groups")
                        .value_name("n")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("1000000")
                        .help("With --hash, fail rather than keep more than this many groups"),
                )
                .arg(
                    Arg::new("allow-unbounded")
                        .long("allow-unbounded")
                        .action(ArgAction::SetTrue)
                        .help("Allow count_distinct and collect, whose memory grows with the input"),
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
                        .help("Keep records where this expression is true, e.g. 'd.a * 2 > d.b and d.state in [\"MN\", \"WI\"]'"),
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
                .about("Aggregates per group key")
                .long_about(
                    "Aggregates per group key, one output record per group.\n\n\
                     By default groups are kept in memory (one set of aggregates per \
                     distinct key, up to --max-groups) and written in key order at the \
                     end. With --sorted, input sorted by the group key is grouped in \
                     constant memory.\n\n\
                     Functions: count sum avg min max first last var var_pop stddev \
                     stddev_pop, and with --allow-unbounded count_distinct collect.",
                )
                .arg(
                    Arg::new("group-by")
                        .short('g')
                        .long("group-by")
                        .help("Selector for a field to group by; repeat for a composite key")
                        .num_args(1)
                        .action(ArgAction::Append)
                        .value_name("selector")
                        .required(true),
                )
                .arg(
                    Arg::new("aggregator")
                        .short('a')
                        .long("agg")
                        .required(true)
                        .num_args(2)
                        .action(ArgAction::Append)
                        .value_names(["aggregator", "selector"])
                        .help("aggregation function along with selector, e.g. -a sum d.salary; repeatable"),
                )
                .arg(
                    Arg::new("sorted")
                        .long("sorted")
                        .action(ArgAction::SetTrue)
                        .help("Input is sorted by the group key: group in constant memory, failing on out-of-order keys"),
                )
                .arg(
                    Arg::new("grouped")
                        .long("grouped")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("sorted")
                        .help("Input has equal keys next to each other, in any order: group in constant memory without checking the order"),
                )
                .arg(
                    Arg::new("max-groups")
                        .long("max-groups")
                        .value_name("n")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("1000000")
                        .help("Fail rather than keep more than this many groups in memory"),
                )
                .arg(
                    Arg::new("allow-unbounded")
                        .long("allow-unbounded")
                        .action(ArgAction::SetTrue)
                        .help("Allow count_distinct and collect, whose memory grows with the input"),
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
        .subcommand(
            Command::new("head")
                .about("Outputs the first n records")
                .arg(lines_arg()),
        )
        .subcommand(
            Command::new("tail")
                .about("Outputs the last n records")
                .arg(lines_arg()),
        )
        .get_matches()
}

fn lines_arg() -> Arg {
    Arg::new("lines")
        .short('n')
        .long("lines")
        .value_name("n")
        .default_value("10")
        .value_parser(clap::value_parser!(usize))
        .help("Number of records")
}
