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

//! The subcommands that evaluate expressions, `select`, `filter` and `sort`,
//! as clap commands and runners that take a [`FunctionRegistry`]. The `ndjson`
//! binary runs them with the built-in functions; `ndjson-spatial` adds its
//! geometry functions.

use crate::{filter, select, sort};
use clap::{Arg, ArgAction, ArgMatches, Command};
use ndjson_common::error::NdJsonSpatialError;
use rmath_ast::FunctionRegistry;
use std::{
    io::{stdin, stdout, BufReader, BufWriter},
    path::PathBuf,
};

pub fn select_command() -> Command {
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
                .help("Allow aggregates whose memory grows with the input, such as count_distinct, collect and union_agg (approx_count_distinct doesn't need this)"),
        )
}

pub fn filter_command() -> Command {
    Command::new("filter")
        .about("returns only json that matches filter expression")
        .arg(
            Arg::new("expression")
                .required(true)
                .help("Keep records where this expression is true, e.g. 'd.a * 2 > d.b and d.state in [\"MN\", \"WI\"]'"),
        )
}

pub fn sort_command() -> Command {
    Command::new("sort")
        .about("Sorts records by one or more keys, spilling to disk when needed")
        .long_about(
            "Sorts records by one or more keys. Records are buffered in memory up to \
             --buffer-size, then sorted runs are written to temporary files and merged. \
             The sort is stable and records are written out unchanged. Keys order as \
             null < false < true < numbers < strings < arrays < objects; a missing \
             field is null.",
        )
        .arg(
            Arg::new("keys")
                .short('k')
                .long("key")
                .short_alias('f')
                .alias("fields")
                .help("Expression to sort by, with :d for descending. e.g. -k d.state -k 'tolower(d.name)' -k d.pop:d")
                .num_args(1..)
                .action(ArgAction::Append)
                .value_name("expr")
                .required(true),
        )
        .arg(
            Arg::new("buffer-size")
                .short('S')
                .long("buffer-size")
                .value_name("size")
                .default_value("256M")
                .value_parser(sort::parse_size)
                .help("Memory for records before spilling to disk: bytes, or with a K, M, G or T suffix"),
        )
        .arg(
            Arg::new("temp-dir")
                .short('T')
                .long("temporary-directory")
                .value_name("dir")
                .value_parser(clap::value_parser!(PathBuf))
                .help("Directory for temporary run files [default: $TMPDIR]"),
        )
        .arg(
            Arg::new("batch-size")
                .long("batch-size")
                .value_name("n")
                .default_value("64")
                .value_parser(clap::value_parser!(u32).range(2..))
                .help("Most runs to merge at once"),
        )
        .arg(
            Arg::new("unique")
                .short('u')
                .long("unique")
                .action(ArgAction::SetTrue)
                .help("Output only the first of records with equal keys"),
        )
        .arg(
            Arg::new("check")
                .short('c')
                .long("check")
                .action(ArgAction::SetTrue)
                .help("Only check that the input is sorted; exit 1 at the first record out of order"),
        )
}

/// Run `filter`; returns the exit code.
pub fn run_filter(args: &ArgMatches, registry: &FunctionRegistry) -> i32 {
    let expression = args
        .get_one::<String>("expression")
        .expect("expression is required");

    match filter::ndjson_filter_with(
        registry,
        expression,
        &mut stdin().lock(),
        &mut stdout().lock(),
    ) {
        Ok(filter::FilterSummary {
            errors,
            first_error: Some((record, err)),
        }) => {
            eprintln!(
                "filter: skipped {} record{} that failed to evaluate (first at record {}: {})",
                errors,
                if errors == 1 { "" } else { "s" },
                record,
                err
            );
            0
        }
        Ok(_) => 0,
        Err(NdJsonSpatialError::Error(err)) => {
            eprintln!("filter: {}", err);
            1
        }
    }
}

/// Run `select`; returns the exit code.
pub fn run_select(args: &ArgMatches, registry: &FunctionRegistry) -> i32 {
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
            return 1;
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
            return 1;
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
    match select::ndjson_select_with(
        registry,
        &list,
        mode,
        &options,
        &mut stdin().lock(),
        &mut output,
    ) {
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
            0
        }
        Err(NdJsonSpatialError::Error(err)) => {
            eprintln!("select: {}", err);
            1
        }
    }
}

/// Run `sort`; returns the exit code.
pub fn run_sort(args: &ArgMatches, registry: &FunctionRegistry) -> i32 {
    let keys = match args
        .get_many::<String>("keys")
        .expect("keys are required")
        .map(|k| sort::SortKey::parse(registry, k))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(keys) => keys,
        Err(NdJsonSpatialError::Error(e)) => {
            eprintln!("sort: {}", e);
            return 2;
        }
    };
    let unique = args.get_flag("unique");
    let mut input = BufReader::with_capacity(1 << 20, stdin().lock());

    if args.get_flag("check") {
        return match sort::check(&mut input, &keys, unique) {
            Ok(None) => 0,
            Ok(Some(n)) => {
                eprintln!("sort: disorder at record {}", n);
                1
            }
            Err(NdJsonSpatialError::Error(e)) => {
                eprintln!("sort: {}", e);
                2
            }
        };
    }

    let mut options = sort::SortOptions::new(keys);
    options.unique = unique;
    options.buffer_size = *args.get_one::<usize>("buffer-size").expect("has a default");
    options.batch_size = *args.get_one::<u32>("batch-size").expect("has a default") as usize;
    options.temp_dir = args.get_one::<PathBuf>("temp-dir").cloned();

    let mut output = BufWriter::with_capacity(1 << 20, stdout().lock());
    match sort::sort(&mut input, &mut output, &options) {
        Ok(summary) => {
            if let Some((n, e)) = summary.first_key_error {
                eprintln!(
                    "sort: {} record{} had a key that failed to evaluate and sorted as null (first at record {}: {})",
                    summary.key_errors,
                    if summary.key_errors == 1 { "" } else { "s" },
                    n,
                    e
                );
            }
            0
        }
        Err(NdJsonSpatialError::Error(e)) => {
            eprintln!("sort: {}", e);
            2
        }
    }
}
