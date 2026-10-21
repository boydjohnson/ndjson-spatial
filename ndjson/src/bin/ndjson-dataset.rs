use clap::{crate_authors, Arg, ArgAction, Command};
use ndjson_common::ndjson::NdjsonReader;
use std::io::Write;

const IRIS: &[u8] = include_bytes!("../../iris.ndjson");

const UCI_CITATION: &str = "Dua, D. and Graff, C. (2019). UCI Machine Learning Repository [http://archive.ics.uci.edu/ml]. Irvine, CA: University of California, School of Information and Computer Science.";

fn main() {
    let mut cmd = cli();
    let args = cmd.get_matches_mut();

    let reader = if let Some(iris_args) = args.subcommand_matches("iris") {
        if iris_args.get_flag("citation") {
            println!("{}", UCI_CITATION);
            std::process::exit(1);
        }
        NdjsonReader::new(IRIS)
    } else {
        println!("{}", cmd.render_usage());
        std::process::exit(1);
    };

    for item in reader.flatten() {
        writeln!(std::io::stdout(), "{}", item).expect("Unable to write to stdout");
    }
}

fn cli() -> Command {
    Command::new("ndjson-dataset")
        .version("0.0.1")
        .author(crate_authors!())
        .about("Example datasets")
        .subcommands(vec![iris_subcommand()])
}

fn iris_subcommand() -> Command {
    Command::new("iris")
        .about("1936 iris dataset from UCI ML Repository")
        .arg(
            Arg::new("citation")
                .long("citation")
                .action(ArgAction::SetTrue),
        )
}
