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

//! `ndjson sort`: an external merge sort over ndjson records.
//!
//! Records are read into a buffer until it holds `buffer_size` bytes, then
//! the buffer is sorted and written to a run file. The runs are k-way merged
//! with a binary heap, at most `batch_size` at a time. Input that fits in
//! one buffer never touches disk.
//!
//! Each record is stored as its sort key, encoded with
//! [`memcomparable`](crate::memcomparable) so runs compare with `memcmp`,
//! next to its original line, which is written out unchanged. The sort is
//! stable: ties keep input order.
//!
//! Run files are unnamed temporary files, so nothing is left behind however
//! the process ends, including on SIGINT.

use crate::{
    filter::legacy_selector,
    head::flush,
    memcomparable,
    records::{for_each_record, io_error, write_record, Flow},
};
use ndjson_common::error::NdJsonSpatialError;
use rmath_ast::{CompileError, CompiledExpr, EvalError, FunctionRegistry};
use serde_json::Value;
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    convert::TryFrom,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    mem,
    path::PathBuf,
};

/// One sort key: an expression and its direction.
#[derive(Debug, Clone)]
pub struct SortKey {
    expr: CompiledExpr,
    descending: bool,
}

impl SortKey {
    /// Parses `expr`, optionally followed by `:a`/`:asc` or `:d`/`:desc`.
    /// The old selector syntax (`d.foo-bar`) is accepted too.
    pub fn parse(registry: &FunctionRegistry, spec: &str) -> Result<Self, NdJsonSpatialError> {
        let (expr, descending) = match spec.rsplit_once(':') {
            Some((expr, "d")) | Some((expr, "desc")) => (expr, true),
            Some((expr, "a")) | Some((expr, "asc")) => (expr, false),
            _ => (spec, false),
        };
        let expr = match registry.compile(expr) {
            Ok(compiled) => compiled,
            Err(err @ CompileError::Parse(_)) => match legacy_selector(expr) {
                Some(path) => CompiledExpr::Select(path),
                None => return Err(invalid_key(spec, err)),
            },
            Err(err) => return Err(invalid_key(spec, err)),
        };
        Ok(SortKey { expr, descending })
    }
}

fn invalid_key(spec: &str, err: CompileError) -> NdJsonSpatialError {
    NdJsonSpatialError::Error(format!("invalid sort key {:?}: {}", spec, err))
}

#[derive(Debug, Clone)]
pub struct SortOptions {
    pub keys: Vec<SortKey>,
    /// Bytes of records (and their keys) to hold in memory before spilling
    /// a run to disk.
    pub buffer_size: usize,
    /// Where run files go; the system temporary directory when `None`.
    pub temp_dir: Option<PathBuf>,
    /// The most runs merged at once, and so roughly the most open run files.
    pub batch_size: usize,
    /// Keep only the first record of each set with equal keys.
    pub unique: bool,
}

pub const DEFAULT_BUFFER_SIZE: usize = 256 * 1024 * 1024;
pub const DEFAULT_BATCH_SIZE: usize = 64;

impl SortOptions {
    pub fn new(keys: Vec<SortKey>) -> Self {
        SortOptions {
            keys,
            buffer_size: DEFAULT_BUFFER_SIZE,
            temp_dir: None,
            batch_size: DEFAULT_BATCH_SIZE,
            unique: false,
        }
    }
}

/// What a sort run noticed along the way.
#[derive(Debug, Default, PartialEq)]
pub struct SortSummary {
    /// Records whose key failed to evaluate; that key sorted as `null`.
    pub key_errors: u64,
    /// The first failure: 1-based record number and the error.
    pub first_key_error: Option<(u64, EvalError)>,
    /// Run files written to disk; 0 when the input fit in memory.
    pub runs: usize,
}

/// Sorts the records of `input` by `options.keys` and writes them to
/// `output`. Blank lines are dropped; a line that isn't JSON is an error.
pub fn sort<I: BufRead, O: Write>(
    input: &mut I,
    output: &mut O,
    options: &SortOptions,
) -> Result<SortSummary, NdJsonSpatialError> {
    let mut sorter = Sorter::new(options);
    let mut summary = SortSummary::default();
    let mut n = 0;

    for_each_record(input, |line| {
        n += 1;
        let key = record_key(&options.keys, line, n, &mut summary)?;
        sorter.push(key, line.to_vec())?;
        Ok(Flow::Continue)
    })?;

    summary.runs = sorter.runs_written;
    let mut last_key: Option<Vec<u8>> = None;
    sorter.finish(|key, line| {
        if options.unique {
            if last_key.as_deref() == Some(key) {
                return Ok(Flow::Continue);
            }
            last_key = Some(key.to_vec());
        }
        Ok(if write_record(output, line)? {
            Flow::Continue
        } else {
            Flow::Stop
        })
    })?;
    flush(output)?;
    Ok(summary)
}

/// Checks that `input` is sorted by `keys` (strictly, with `unique`) and
/// returns the 1-based number of the first record out of order.
pub fn check<I: BufRead>(
    input: &mut I,
    keys: &[SortKey],
    unique: bool,
) -> Result<Option<u64>, NdJsonSpatialError> {
    let mut previous: Option<Vec<u8>> = None;
    let mut disorder = None;
    let mut n = 0;

    for_each_record(input, |line| {
        n += 1;
        let key = record_key(keys, line, n, &mut SortSummary::default())?;
        if let Some(previous) = &previous {
            if key < *previous || (unique && key == *previous) {
                disorder = Some(n);
                return Ok(Flow::Stop);
            }
        }
        previous = Some(key);
        Ok(Flow::Continue)
    })?;
    Ok(disorder)
}

pub(crate) fn record_key(
    keys: &[SortKey],
    line: &[u8],
    n: u64,
    summary: &mut SortSummary,
) -> Result<Vec<u8>, NdJsonSpatialError> {
    let record: Value = serde_json::from_slice(line)
        .map_err(|e| NdJsonSpatialError::Error(format!("record {}: invalid JSON: {}", n, e)))?;
    let mut encoded = Vec::new();
    let mut failed = false;
    for key in keys {
        let value = key.expr.eval(&record).unwrap_or_else(|e| {
            summary.first_key_error.get_or_insert((n, e));
            failed = true;
            Value::Null
        });
        memcomparable::encode(&value, key.descending, &mut encoded);
    }
    if failed {
        summary.key_errors += 1;
    }
    Ok(encoded)
}

struct Entry {
    key: Vec<u8>,
    line: Vec<u8>,
}

/// A sorted run on disk, tagged with how many merges built it.
struct Run {
    level: u32,
    file: File,
}

struct Sorter<'a> {
    options: &'a SortOptions,
    buffer: Vec<Entry>,
    buffered_bytes: usize,
    /// Runs in input order. Levels never increase along the list, so the
    /// runs of the newest level are always a contiguous tail.
    runs: Vec<Run>,
    runs_written: usize,
}

impl<'a> Sorter<'a> {
    fn new(options: &'a SortOptions) -> Self {
        Sorter {
            options,
            buffer: Vec::new(),
            buffered_bytes: 0,
            runs: Vec::new(),
            runs_written: 0,
        }
    }

    fn push(&mut self, key: Vec<u8>, line: Vec<u8>) -> Result<(), NdJsonSpatialError> {
        self.buffered_bytes += key.len() + line.len() + mem::size_of::<Entry>();
        self.buffer.push(Entry { key, line });
        if self.buffered_bytes >= self.options.buffer_size {
            self.spill()?;
        }
        Ok(())
    }

    /// Sorts the buffer into a new run, then merges the newest runs while
    /// `batch_size` of them share a level, so open files stay bounded.
    fn spill(&mut self) -> Result<(), NdJsonSpatialError> {
        let mut buffer = mem::take(&mut self.buffer);
        self.buffered_bytes = 0;
        buffer.sort_by(|a, b| a.key.cmp(&b.key));

        let mut writer = self.run_writer()?;
        for entry in &buffer {
            write_entry(&mut writer, &entry.key, &entry.line)?;
        }
        let file = finish_run(writer)?;
        self.runs.push(Run { level: 0, file });

        let batch = self.batch_size();
        loop {
            let level = self.runs.last().map_or(0, |r| r.level);
            let tail = self.runs.iter().rev().take_while(|r| r.level == level);
            if tail.count() < batch {
                return Ok(());
            }
            let start = self.runs.len() - batch;
            let merged = self.runs.drain(start..).map(|r| r.file).collect();
            let file = self.merge_to_run(merged)?;
            self.runs.push(Run {
                level: level + 1,
                file,
            });
        }
    }

    fn batch_size(&self) -> usize {
        self.options.batch_size.max(2)
    }

    fn run_writer(&mut self) -> Result<BufWriter<File>, NdJsonSpatialError> {
        let file = match &self.options.temp_dir {
            Some(dir) => tempfile::tempfile_in(dir),
            None => tempfile::tempfile(),
        }
        .map_err(|e| NdJsonSpatialError::Error(format!("unable to create a run file: {}", e)))?;
        self.runs_written += 1;
        Ok(BufWriter::with_capacity(1 << 20, file))
    }

    fn merge_to_run(&mut self, runs: Vec<File>) -> Result<File, NdJsonSpatialError> {
        let mut writer = self.run_writer()?;
        merge(runs, |key, line| {
            write_entry(&mut writer, key, line)?;
            Ok(Flow::Continue)
        })?;
        finish_run(writer)
    }

    /// Calls `emit` with every record in order.
    fn finish<F>(mut self, mut emit: F) -> Result<(), NdJsonSpatialError>
    where
        F: FnMut(&[u8], &[u8]) -> Result<Flow, NdJsonSpatialError>,
    {
        if self.runs.is_empty() {
            self.buffer.sort_by(|a, b| a.key.cmp(&b.key));
            for entry in &self.buffer {
                if let Flow::Stop = emit(&entry.key, &entry.line)? {
                    break;
                }
            }
            return Ok(());
        }

        if !self.buffer.is_empty() {
            self.spill()?;
        }
        let batch = self.batch_size();
        let mut runs: Vec<File> = mem::take(&mut self.runs)
            .into_iter()
            .map(|r| r.file)
            .collect();
        while runs.len() > batch {
            let mut merged = Vec::with_capacity(runs.len() / batch + 1);
            let mut runs_iter = runs.into_iter().peekable();
            while runs_iter.peek().is_some() {
                let group: Vec<File> = runs_iter.by_ref().take(batch).collect();
                merged.push(if group.len() == 1 {
                    group.into_iter().next().expect("one run")
                } else {
                    self.merge_to_run(group)?
                });
            }
            runs = merged;
        }
        merge(runs, emit)
    }
}

/// The smallest unread record of one run. Ties on `key` go to the earlier
/// run, which keeps the merge stable.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Head {
    key: Vec<u8>,
    run: usize,
    line: Vec<u8>,
}

/// K-way merges `runs`, which are in input order, into `sink`.
fn merge<F>(runs: Vec<File>, mut sink: F) -> Result<(), NdJsonSpatialError>
where
    F: FnMut(&[u8], &[u8]) -> Result<Flow, NdJsonSpatialError>,
{
    let mut readers: Vec<BufReader<File>> = runs
        .into_iter()
        .map(|f| BufReader::with_capacity(1 << 16, f))
        .collect();
    let mut heap = BinaryHeap::with_capacity(readers.len());
    for (run, reader) in readers.iter_mut().enumerate() {
        let mut head = Head {
            key: Vec::new(),
            run,
            line: Vec::new(),
        };
        if read_entry(reader, &mut head.key, &mut head.line)? {
            heap.push(Reverse(head));
        }
    }

    while let Some(Reverse(mut head)) = heap.pop() {
        if let Flow::Stop = sink(&head.key, &head.line)? {
            return Ok(());
        }
        if read_entry(&mut readers[head.run], &mut head.key, &mut head.line)? {
            heap.push(Reverse(head));
        }
    }
    Ok(())
}

// A run file is a sequence of entries: key length (u32 LE), key, line
// length (u32 LE), line.

fn write_entry<W: Write>(w: &mut W, key: &[u8], line: &[u8]) -> Result<(), NdJsonSpatialError> {
    write_chunk(w, key)?;
    write_chunk(w, line)
}

fn write_chunk<W: Write>(w: &mut W, bytes: &[u8]) -> Result<(), NdJsonSpatialError> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        NdJsonSpatialError::Error(format!("record of {} bytes is too long", bytes.len()))
    })?;
    w.write_all(&len.to_le_bytes())
        .and_then(|_| w.write_all(bytes))
        .map_err(io_error)
}

fn finish_run(writer: BufWriter<File>) -> Result<File, NdJsonSpatialError> {
    let mut file = writer.into_inner().map_err(|e| io_error(e.into_error()))?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok(file)
}

/// Reads the next entry into `key` and `line`; `Ok(false)` at the end.
fn read_entry<R: BufRead>(
    r: &mut R,
    key: &mut Vec<u8>,
    line: &mut Vec<u8>,
) -> Result<bool, NdJsonSpatialError> {
    if r.fill_buf().map_err(io_error)?.is_empty() {
        return Ok(false);
    }
    read_chunk(r, key)?;
    read_chunk(r, line)?;
    Ok(true)
}

fn read_chunk<R: Read>(r: &mut R, out: &mut Vec<u8>) -> Result<(), NdJsonSpatialError> {
    let mut len = [0; 4];
    r.read_exact(&mut len).map_err(io_error)?;
    out.clear();
    out.resize(u32::from_le_bytes(len) as usize, 0);
    r.read_exact(out).map_err(io_error)
}

/// Parses a buffer size: bytes, or a number with a `K`, `M`, `G` or `T`
/// suffix (powers of 1024), optionally followed by `B` or `iB`.
pub fn parse_size(s: &str) -> Result<usize, String> {
    let upper = s.trim().to_ascii_uppercase();
    let upper = upper
        .strip_suffix("IB")
        .or_else(|| upper.strip_suffix('B'))
        .unwrap_or(&upper);
    let (digits, shift) = match upper.char_indices().last() {
        Some((i, 'K')) => (&upper[..i], 10),
        Some((i, 'M')) => (&upper[..i], 20),
        Some((i, 'G')) => (&upper[..i], 30),
        Some((i, 'T')) => (&upper[..i], 40),
        _ => (upper, 0),
    };
    let n: usize = digits
        .parse()
        .map_err(|_| format!("invalid size {:?}: expected e.g. 4096, 512K, 256M, 2G", s))?;
    n.checked_mul(1 << shift)
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("invalid size {:?}", s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(specs: &[&str]) -> Vec<SortKey> {
        let registry = FunctionRegistry::default();
        specs
            .iter()
            .map(|s| SortKey::parse(&registry, s).unwrap())
            .collect()
    }

    fn run(input: &str, options: &SortOptions) -> (String, SortSummary) {
        let mut output = vec![];
        let summary = sort(&mut input.as_bytes(), &mut output, options).unwrap();
        (String::from_utf8(output).unwrap(), summary)
    }

    fn sorted(input: &str, specs: &[&str]) -> String {
        run(input, &SortOptions::new(keys(specs))).0
    }

    #[test]
    fn test_sort_already_sorted() {
        let input = "{\"bar\":3,\"foo\":4}\n{\"bar\":4,\"foo\":7}\n";
        assert_eq!(sorted(input, &["d.bar"]), input);
        assert_eq!(sorted(input, &["d.foo", "d.bar"]), input);
    }

    #[test]
    fn test_sort_descending() {
        let input = "{\"bar\":3,\"foo\":4}\n{\"bar\":4,\"foo\":7}\n";
        assert_eq!(
            sorted(input, &["d.bar:d"]),
            "{\"bar\":4,\"foo\":7}\n{\"bar\":3,\"foo\":4}\n"
        );
        assert_eq!(sorted(input, &["d.bar:desc"]), sorted(input, &["d.bar:d"]));
    }

    #[test]
    fn test_sort_numbers_numerically() {
        let input = "{\"a\":3}\n{\"a\":-1}\n{\"a\":2.5}\n{\"a\":10}\n";
        assert_eq!(
            sorted(input, &["d.a"]),
            "{\"a\":-1}\n{\"a\":2.5}\n{\"a\":3}\n{\"a\":10}\n"
        );
    }

    #[test]
    fn test_sort_mixed_directions_and_missing_keys() {
        let input =
            "{\"s\":\"b\",\"n\":1}\n{\"s\":\"a\",\"n\":1}\n{\"n\":2}\n{\"s\":\"c\",\"n\":2}\n";
        assert_eq!(
            sorted(input, &["d.n:d", "d.s"]),
            "{\"n\":2}\n{\"s\":\"c\",\"n\":2}\n{\"s\":\"a\",\"n\":1}\n{\"s\":\"b\",\"n\":1}\n"
        );
    }

    #[test]
    fn test_sort_keeps_lines_unchanged() {
        let input = "{ \"z\": 2, \"a\": 1 }\n\n{\"z\":1}\n";
        assert_eq!(
            sorted(input, &["d.z"]),
            "{\"z\":1}\n{ \"z\": 2, \"a\": 1 }\n"
        );
    }

    #[test]
    fn test_sort_by_expression() {
        let input = "{\"n\":\"b\"}\n{\"n\":\"C\"}\n{\"n\":\"a\"}\n";
        assert_eq!(
            sorted(input, &["d.n"]),
            "{\"n\":\"C\"}\n{\"n\":\"a\"}\n{\"n\":\"b\"}\n"
        );
        assert_eq!(
            sorted(input, &["tolower(d.n)"]),
            "{\"n\":\"a\"}\n{\"n\":\"b\"}\n{\"n\":\"C\"}\n"
        );
        let input = "{\"x\":2}\n{\"x\":3}\n{\"x\":1}\n";
        assert_eq!(sorted(input, &["-d.x"]), sorted(input, &["d.x:d"]));
    }

    #[test]
    fn test_sort_legacy_selector() {
        let input = "{\"foo-bar\":2}\n{\"foo-bar\":1}\n";
        assert_eq!(
            sorted(input, &["d.foo-bar"]),
            "{\"foo-bar\":1}\n{\"foo-bar\":2}\n"
        );
    }

    #[test]
    fn test_invalid_key_and_json() {
        let registry = FunctionRegistry::default();
        assert!(SortKey::parse(&registry, "d.a +").is_err());
        assert!(SortKey::parse(&registry, "nosuchfn(d.a)").is_err());

        let mut output = vec![];
        let err = sort(
            &mut "{\"a\":1}\nnot json\n".as_bytes(),
            &mut output,
            &SortOptions::new(keys(&["d.a"])),
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("record 2"));
    }

    #[test]
    fn test_key_errors_sort_as_null() {
        let input = "{\"a\":1}\n{\"a\":\"x\"}\n{\"a\":0}\n";
        let (out, summary) = run(input, &SortOptions::new(keys(&["d.a * 2"])));
        assert_eq!(out, "{\"a\":\"x\"}\n{\"a\":0}\n{\"a\":1}\n");
        assert_eq!(summary.key_errors, 1);
        assert_eq!(summary.first_key_error.map(|(n, _)| n), Some(2));
    }

    /// Records with a key and their input position, shuffled.
    fn shuffled(n: usize) -> (String, Vec<(u64, usize)>) {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut records = Vec::new();
        let mut input = String::new();
        for i in 0..n {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let k = state % 97;
            records.push((k, i));
            input.push_str(&format!("{{\"k\":{},\"i\":{}}}\n", k, i));
        }
        (input, records)
    }

    fn expected(mut records: Vec<(u64, usize)>, descending: bool) -> String {
        if descending {
            records.sort_by_key(|r| std::cmp::Reverse(r.0));
        } else {
            records.sort_by_key(|r| r.0);
        }
        records
            .iter()
            .map(|(k, i)| format!("{{\"k\":{},\"i\":{}}}\n", k, i))
            .collect()
    }

    #[test]
    fn test_external_sort_is_stable_across_runs() {
        let (input, records) = shuffled(2000);
        for &(buffer_size, batch_size) in &[(1, 2), (500, 3), (4096, 64), (usize::MAX, 64)] {
            for &descending in &[false, true] {
                let mut options =
                    SortOptions::new(keys(&[if descending { "d.k:d" } else { "d.k" }]));
                options.buffer_size = buffer_size;
                options.batch_size = batch_size;
                let (out, summary) = run(&input, &options);
                assert_eq!(
                    out,
                    expected(records.clone(), descending),
                    "buffer {} batch {}",
                    buffer_size,
                    batch_size
                );
                assert_eq!(summary.runs == 0, buffer_size == usize::MAX);
            }
        }
    }

    #[test]
    fn test_unique_keeps_first() {
        let (input, records) = shuffled(500);
        let mut options = SortOptions::new(keys(&["d.k"]));
        options.unique = true;
        let want: String = {
            let mut seen = std::collections::HashSet::new();
            let mut firsts: Vec<_> = records.into_iter().filter(|r| seen.insert(r.0)).collect();
            firsts.sort_by_key(|r| r.0);
            firsts
                .iter()
                .map(|(k, i)| format!("{{\"k\":{},\"i\":{}}}\n", k, i))
                .collect()
        };
        assert_eq!(run(&input, &options).0, want);
        options.buffer_size = 64;
        options.batch_size = 2;
        assert_eq!(run(&input, &options).0, want);
    }

    #[test]
    fn test_temp_dir_is_left_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (input, records) = shuffled(300);
        let mut options = SortOptions::new(keys(&["d.k"]));
        options.buffer_size = 100;
        options.batch_size = 4;
        options.temp_dir = Some(dir.path().to_path_buf());
        let (out, summary) = run(&input, &options);
        assert_eq!(out, expected(records, false));
        assert!(summary.runs > 4);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn test_check() {
        let k = keys(&["d.a"]);
        assert_eq!(check(&mut "".as_bytes(), &k, false).unwrap(), None);
        let input = "{\"a\":1}\n{\"a\":1}\n{\"a\":2}\n";
        assert_eq!(check(&mut input.as_bytes(), &k, false).unwrap(), None);
        assert_eq!(check(&mut input.as_bytes(), &k, true).unwrap(), Some(2));
        let input = "{\"a\":1}\n\n{\"a\":3}\n{\"a\":2}\n";
        assert_eq!(check(&mut input.as_bytes(), &k, false).unwrap(), Some(3));
        let k = keys(&["d.a:d"]);
        assert_eq!(check(&mut input.as_bytes(), &k, false).unwrap(), Some(2));
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("4096"), Ok(4096));
        assert_eq!(parse_size("512K"), Ok(512 << 10));
        assert_eq!(parse_size("256m"), Ok(256 << 20));
        assert_eq!(parse_size("2GiB"), Ok(2 << 30));
        assert_eq!(parse_size("1MB"), Ok(1 << 20));
        assert!(parse_size("0").is_err());
        assert!(parse_size("lots").is_err());
        assert!(parse_size("M").is_err());
    }
}
