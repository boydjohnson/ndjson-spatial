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
    cli, from_json,
    group::{AggregateSummary, Grouping},
    head, join, pick_field, tail,
};
use ndjson_common::{error::NdJsonSpatialError, json_selector_parser::parse_json_selector};
use rmath_ast::FunctionRegistry;
use std::{
    fs::File,
    io::{stdin, stdout, BufRead, BufReader, BufWriter, Write},
    process::exit,
};

fn main() {
    let args = parse_args();

    let registry = FunctionRegistry::default();
    if let Some(args) = args.subcommand_matches("filter") {
        exit(cli::run_filter(args, &registry));
    } else if let Some(args) = args.subcommand_matches("select") {
        exit(cli::run_select(args, &registry));
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
    } else if let Some(args) = args.subcommand_matches("join") {
        exit(run_join(args));
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
        exit(cli::run_sort(args, &registry));
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

fn run_join(args: &ArgMatches) -> i32 {
    match join_command(args) {
        Ok(summary) => {
            if let Some(first) = summary.first_key_error {
                eprintln!(
                    "join: {} record{} had a key that failed to evaluate and never matched \
                     (first at {} line {}: {})",
                    summary.key_errors,
                    if summary.key_errors == 1 { "" } else { "s" },
                    first.input,
                    first.line,
                    first.error
                );
            }
            0
        }
        Err(NdJsonSpatialError::Error(err)) => {
            eprintln!("join: {}", err);
            1
        }
    }
}

fn join_command(args: &ArgMatches) -> Result<join::JoinSummary, NdJsonSpatialError> {
    let error = |message: &str| NdJsonSpatialError::Error(message.to_owned());
    let string = |name: &str| args.get_one::<String>(name).map(String::as_str);

    let (left_on, right_on) = if let Some(on) = string("on") {
        (join::parse_keys("--on", on)?, join::parse_keys("--on", on)?)
    } else if let (Some(left), Some(right)) = (string("left-on"), string("right-on")) {
        (
            join::parse_keys("--left-on", left)?,
            join::parse_keys("--right-on", right)?,
        )
    } else if let (Some(reference), Some(stream)) = (
        args.get_many::<String>("reference-fields"),
        args.get_many::<String>("stream-fields"),
    ) {
        eprintln!(
            "join: --ref-fields and --stream-fields are deprecated; use --right-on and --left-on \
             with comma-separated keys"
        );
        (
            stream
                .map(|s| join::parse_legacy_key("--stream-fields", s))
                .collect::<Result<_, _>>()?,
            reference
                .map(|s| join::parse_legacy_key("--ref-fields", s))
                .collect::<Result<_, _>>()?,
        )
    } else {
        return Err(error(
            "give the join key with --on, or --left-on and --right-on",
        ));
    };

    let mut options = join::JoinOptions::new(left_on, right_on)?;
    options.kind = [
        ("left", join::JoinType::Left),
        ("right", join::JoinType::Right),
        ("outer", join::JoinType::Outer),
        ("semi", join::JoinType::Semi),
        ("anti", join::JoinType::Anti),
    ]
    .iter()
    .find(|(flag, _)| args.get_flag(flag))
    .map_or(join::JoinType::Inner, |(_, kind)| *kind);
    if let Some(prefix) = string("prefix-right") {
        options.right_fields = join::RightFields::Prefix(prefix.to_owned());
    } else if let Some(path) = string("into") {
        options.right_fields = join::RightFields::Into(join::parse_into(path)?);
    }
    options.max_run = *args
        .get_one::<usize>("max-run")
        .expect("max-run has a default");

    let left_name = string("left-input").unwrap_or("-");
    let mut output = BufWriter::with_capacity(1_000_000, stdout().lock());
    if let Some(reference) = string("reference") {
        if string("right-input").is_some() {
            return Err(error(
                "with --ref, give at most one input: the --ref file is the right side",
            ));
        }
        join::hash_join(
            join::Input::new(display_name(left_name), open_input(left_name)?),
            join::Input::new(reference, open_input(reference)?),
            &mut output,
            &options,
        )
    } else {
        let right_name = match string("right-input") {
            Some(right) => right,
            None => return Err(error("give two inputs, LEFT and RIGHT, or use --ref")),
        };
        if left_name == "-" && right_name == "-" {
            return Err(error("only one input can be stdin"));
        }
        join::merge_join(
            join::Input::new(display_name(left_name), open_input(left_name)?),
            join::Input::new(display_name(right_name), open_input(right_name)?),
            &mut output,
            &options,
        )
    }
}

fn display_name(name: &str) -> &str {
    if name == "-" {
        "stdin"
    } else {
        name
    }
}

/// Opens `name` for reading, or stdin for `-`.
fn open_input(name: &str) -> Result<Box<dyn BufRead>, NdJsonSpatialError> {
    if name == "-" {
        return Ok(Box::new(BufReader::with_capacity(1 << 20, stdin())));
    }
    File::open(name)
        .map(|file| Box::new(BufReader::with_capacity(1 << 20, file)) as Box<dyn BufRead>)
        .map_err(|e| NdJsonSpatialError::Error(format!("unable to open {}: {}", name, e)))
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
        .subcommand(cli::select_command())
        .subcommand(
            Command::new("join")
                .about("Joins two ndjson inputs on a key")
                .long_about(
                    "Joins two ndjson inputs on a key. By default this is a merge join: both \
                     inputs must be sorted ascending by their keys (ndjson sort -k ...), memory \
                     holds only the right records of one key, and a record out of order is an \
                     error. With --ref the reference file is loaded into memory and the left \
                     input (stdin by default) can be in any order. Keys with a null, array or \
                     object component never match. On field collisions the right value wins, \
                     unless --prefix-right or --into is given.",
                )
                .arg(
                    Arg::new("left-input")
                        .value_name("LEFT")
                        .help("The left input, '-' for stdin; with --ref, defaults to stdin"),
                )
                .arg(
                    Arg::new("right-input")
                        .value_name("RIGHT")
                        .help("The right input, '-' for stdin; not given with --ref"),
                )
                .arg(
                    Arg::new("on")
                        .long("on")
                        .value_name("keys")
                        .conflicts_with_all(["left-on", "right-on"])
                        .help("Comma-separated key expressions for both sides, matched as a tuple, e.g. --on 'd.state, d.county'"),
                )
                .arg(
                    Arg::new("left-on")
                        .long("left-on")
                        .value_name("keys")
                        .requires("right-on")
                        .help("Key expressions for the left input"),
                )
                .arg(
                    Arg::new("right-on")
                        .long("right-on")
                        .value_name("keys")
                        .requires("left-on")
                        .help("Key expressions for the right input"),
                )
                .arg(
                    Arg::new("reference")
                        .short('r')
                        .long("ref")
                        .value_name("file")
                        .help("Hash join: load this file into memory as the right side"),
                )
                .arg(
                    Arg::new("left")
                        .long("left")
                        .action(ArgAction::SetTrue)
                        .help("Also write left records without a match, unchanged"),
                )
                .arg(
                    Arg::new("right")
                        .long("right")
                        .action(ArgAction::SetTrue)
                        .help("Also write right records without a match (merge join only)"),
                )
                .arg(
                    Arg::new("outer")
                        .long("outer")
                        .action(ArgAction::SetTrue)
                        .help("Also write records without a match from both sides (merge join only)"),
                )
                .arg(
                    Arg::new("semi")
                        .long("semi")
                        .action(ArgAction::SetTrue)
                        .help("Write left records that have a match, once each, unchanged"),
                )
                .arg(
                    Arg::new("anti")
                        .long("anti")
                        .action(ArgAction::SetTrue)
                        .help("Write left records that have no match, unchanged"),
                )
                .group(
                    clap::ArgGroup::new("join-type")
                        .args(["left", "right", "outer", "semi", "anti"])
                        .multiple(false),
                )
                .arg(
                    Arg::new("prefix-right")
                        .long("prefix-right")
                        .value_name("prefix")
                        .conflicts_with("into")
                        .help("Prefix the right record's field names, e.g. --prefix-right r_"),
                )
                .arg(
                    Arg::new("into")
                        .long("into")
                        .value_name("path")
                        .help("Nest the whole right record at this path, e.g. --into d.right"),
                )
                .arg(
                    Arg::new("max-run")
                        .long("max-run")
                        .value_name("n")
                        .value_parser(clap::value_parser!(usize))
                        .default_value("1000000")
                        .help("Merge join: fail rather than hold more than this many right records with one key"),
                )
                .arg(
                    Arg::new("reference-fields")
                        .long("ref-fields")
                        .num_args(1..)
                        .requires("reference")
                        .requires("stream-fields")
                        .conflicts_with_all(["on", "left-on", "right-on"])
                        .help("Deprecated: use --right-on. Selectors for the --ref file"),
                )
                .arg(
                    Arg::new("stream-fields")
                        .long("stream-fields")
                        .num_args(1..)
                        .requires("reference-fields")
                        .help("Deprecated: use --left-on. Selectors for the stream"),
                ),
        )
        .subcommand(cli::filter_command())
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
                     stddev_pop median approx_count_distinct, and with --allow-unbounded \
                     count_distinct collect.",
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
                        .help("Allow count_distinct and collect, whose memory grows with the input (approx_count_distinct doesn't need this)"),
                ),
        )
        .subcommand(cli::sort_command())
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
