extern crate geo;
extern crate geojson;

use crate::error::NdJsonSpatialError;
use geojson::GeoJson;
use std::io::Write;

pub struct NdJsonReader {}

impl NdJsonReader {
    pub fn new() -> Self {
        NdJsonReader {}
    }
}

impl Default for NdJsonReader {
    fn default() -> Self {
        Self::new()
    }
}

impl Iterator for NdJsonReader {
    type Item = Result<GeoJson, NdJsonSpatialError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match ::std::io::stdin().read_line(&mut line) {
            Ok(_) => {
                if line.is_empty() {
                    return None;
                }
                let line = if line.ends_with(",") {
                    let comma_index = line.len() - 1;
                    line[..comma_index].to_string()
                } else {
                    line
                };
                Some(
                    line.parse::<GeoJson>()
                        .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e))),
                )
            }
            Err(e) => {
                let error_msg = format!("Error reading ndjson: {}", e);

                if !::std::io::stderr()
                    .write(error_msg.as_bytes().as_ref())
                    .is_ok()
                {
                    panic!("Error writing to stderr: {}", error_msg);
                }
                None
            }
        }
    }
}
