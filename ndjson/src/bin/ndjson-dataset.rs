use clap::{crate_authors, App, Arg, ArgMatches, SubCommand};
use ndjson_common::ndjson::NdjsonReader;
use std::io::Write;

const IRIS: &[u8] = include_bytes!("../../iris.ndjson");

const UCI_CITATION: &str = "Dua, D. and Graff, C. (2019). UCI Machine Learning Repository [http://archive.ics.uci.edu/ml]. Irvine, CA: University of California, School of Information and Computer Science.";

fn main() {
    let args = parse_args();

    let reader = if let Some(iris_args) = args.subcommand_matches("iris") {
        if iris_args.is_present("citation") {
            println!("{}", UCI_CITATION);
            std::process::exit(1);
        }
        NdjsonReader::new(IRIS)
    } else {
        println!("{}", args.usage());
        std::process::exit(1);
    };

    for item in reader.flatten() {
        writeln!(std::io::stdout(), "{}", item).expect("Unable to write to stdout");
    }
}

fn parse_args<'a>() -> ArgMatches<'a> {
    App::new("ndjson-dataset")
        .version("0.0.1")
        .author(crate_authors!())
        .about("Example datasets")
        .subcommands(vec![iris_subcommand()])
        .get_matches()
}

fn iris_subcommand<'a>() -> App<'a, 'a> {
    SubCommand::with_name("iris")
        .about("1936 iris dataset from UCI ML Repository")
        .arg(
            Arg::with_name("citation")
                .long("citation")
                .takes_value(false),
        )
}
