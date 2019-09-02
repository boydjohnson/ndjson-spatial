use crate::error::NdJsonSpatialError;
use crate::filter::select_from_json_object;
use crate::json_parser::parse_json_selector;
use crate::ndjson::NdjsonReader;
use serde_json::ser::to_string;
use serde_json::Value;
use std::io::Write;

pub fn pick_field(expression: &str) -> Result<(), NdJsonSpatialError> {
    let (_, identifiers) = parse_json_selector(expression)
        .map_err(|e| NdJsonSpatialError::Error(format!("Unable to parse expression: {}", e)))?;
    for value in NdjsonReader::default() {
        let v = value?;
        if let Ok(value) = select_from_json_object(v.clone(), &identifiers) {
            match value {
                Value::String(s) => {
                    writeln!(::std::io::stdout(), "{}", s).expect("Unable to write to stdout");
                }
                Value::Number(n) => {
                    writeln!(::std::io::stdout(), "{}", n).expect("Unable to write to stdout");
                }
                Value::Object(o) => {
                    writeln!(
                        ::std::io::stdout(),
                        "{}",
                        to_string(&o).expect("ndjson object failed serialazation")
                    )
                    .expect("Unable to write to stdout");
                }
                Value::Bool(b) => {
                    writeln!(::std::io::stdout(), "{}", b).expect("Unable to write to stdout");
                }
                Value::Array(a) => {
                    for item in &a {
                        writeln!(::std::io::stdout(), "{}", item)
                            .expect("Unable to write to stdout");
                    }
                }
                _ => (),
            }
        }
    }
    Ok(())
}
