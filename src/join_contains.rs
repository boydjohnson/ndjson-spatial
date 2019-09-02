use crate::error::NdJsonSpatialError;
use crate::ndjson::NdJsonGeojsonReader;
use std::fs::File;

pub fn join_contains(mut reference_file: File, field_name: &str) -> Result<(), NdJsonSpatialError> {
    Ok(())
}
