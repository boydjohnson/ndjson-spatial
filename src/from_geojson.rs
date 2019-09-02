use crate::error::NdJsonSpatialError;
use std::io::{Read, Write};

pub fn split() -> Result<(), NdJsonSpatialError> {
    let mut string_until_feature = String::new();

    for byte in ::std::io::stdin().bytes() {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();

        string_until_feature.push(c);
        if string_until_feature.contains("features") {
            break;
        }
    }
    let mut json_string = String::new();
    let mut start = false;
    let mut count = 0;
    for byte in ::std::io::stdin().bytes() {
        let c = byte
            .map_err(|e| NdJsonSpatialError::Error(format!("{:?}", e)))?
            .into();
        if c == '{' {
            count += 1;
            start = true;
        }
        if start && c != '\n' {
            json_string.push(c);
        }

        if c == '}' {
            count -= 1;
        }
        if count == 0 && start {
            writeln!(::std::io::stdout(), "{}", json_string).expect("Error writing to stdout");
            json_string = String::new();
            start = false;
        }
    }
    Ok(())
}
