use csv::ReaderBuilder;
use ndjson_common::{error::NdJsonSpatialError, infer_json::infer_json};
use serde_json::{Map, Value};
use std::io::Write;

pub fn from_csv(delimiter: u8) -> Result<(), NdJsonSpatialError> {
    let mut reader = ReaderBuilder::new()
        .delimiter(delimiter)
        .from_reader(std::io::stdin());

    let headers = match reader.headers() {
        Ok(record) => record.clone(),
        Err(e) => {
            return Err(NdJsonSpatialError::Error(format!(
                "During read from csv headers: {}",
                e
            )));
        }
    };

    for row in reader.records() {
        match row {
            Ok(row) => {
                let map = row.iter().enumerate().fold(
                    Map::new(),
                    |mut map: Map<String, Value>, (idx, row_item)| {
                        if let Some(head) = headers.get(idx) {
                            map.insert(head.to_string(), infer_json(row_item));
                        }
                        map
                    },
                );
                if let Ok(json) = serde_json::to_string(&map) {
                    writeln!(std::io::stdout(), "{}", json).expect("Unable to write to stdout");
                }
            }
            Err(e) => {
                writeln!(std::io::stderr(), "{}", e).expect("Unable to write to stderr");
            }
        }
    }

    Ok(())
}
