# Sort and join

Today both are S∞: `sort` collects the stream with `itertools::sorted_by`;
`join` loads the reference file into a `BTreeMap`. Plan: make `sort` an
external sort, make the default `join` a merge join over sorted inputs, and
keep the in-memory hash join behind the explicit `--ref` flag.

## Value ordering

One total order over JSON values, used by `sort`, `--by`, merge `join`, `top`,
`min`/`max`:

`null < false < true < numbers < strings < arrays < objects`

- Numbers compare **numerically** across i64/u64/f64. Today `OrderedNumber`
  derives `Ord` on the enum, so all `PosInt` < all `NegInt` < all `Float`
  (`-1` sorts after `5`; `2.5` sorts after `3`). Fix this first — it affects
  current `sort`, `agg`, and `join`.
- `NaN` sorts after all other numbers (and equals itself for grouping).
- Strings: byte-wise (UTF-8) by default; `--locale`-style collation is out of
  scope.
- Arrays: lexicographic. Objects: by sorted (key, value) pairs.

## `sort` — external merge sort

```sh
ndjson sort -k d.state -k d.pop:d [-S 512M] [-T /tmp] [--stable] [--check]
```

1. Read records into a buffer until its encoded size hits the budget
   (`-S`, default 256 MiB).
2. Sort the buffer by the extracted key tuple; write a run file to `-T`
   (default `$TMPDIR`). Store each record as `key-bytes \t original-line`
   so merging never re-parses JSON, and re-emit the original line unchanged.
3. K-way merge the runs with a binary heap. If the run count exceeds
   `--batch-size` (default 64 open files), merge in passes.
4. Inputs that fit in one buffer never touch disk (same speed as today).

- Sort keys are full expressions (`-k 'tolower(d.name)'`), `:d` for
  descending (existing syntax).
- Stability: ties keep input order (run index + sequence number).
- `--check`: S0, exit non-zero at the first out-of-order line.
- `-u`: drop records with equal keys (keep first).
- Key encoding: an order-preserving byte encoding of the value tuple
  (memcomparable, like FoundationDB tuple layer) lets the merge compare with
  `memcmp` and lets `--check`/`--by` compare cheaply.
- Temp files are deleted on success, on error, and on SIGINT.

Space: O(budget) memory, O(n) disk.

Status: done. The sort is always stable, so there is no `--stable` flag.
Run files are length-prefixed `key, line` pairs rather than tab-separated,
since encoded keys are binary. The key encoding lives in
`ndjson/src/memcomparable.rs` for merge join and `--by` to reuse. A key that
fails to evaluate (`d.s * 2` on a string) sorts as `null` and is counted on
stderr; a missing field is `null`. Run files are unnamed temp files, so
SIGINT and SIGKILL leave nothing behind. Merging is tiered: once
`--batch-size` runs share a level they merge into one, which bounds open
files while reading.

## `join`

### Merge join (default) — S0

```sh
ndjson join left.ndjson right.ndjson --on d.state,d.county
cat left.ndjson | ndjson join - right.ndjson --left-on d.st --right-on d.state
```

Both inputs sorted by the join key (in the order defined above). Advance the
side with the smaller key; on equal keys, buffer the current run from **one
side** and stream the other. Memory is bounded by the largest single-key
run (documented; `--max-run N` guards it).

- Composite keys are compared as a tuple (fixes today's behaviour, where each
  `--ref-fields`/`--stream-fields` pair is joined independently, and the
  reference reader is reused so only the first pair sees any data).
- Join types: inner (default), `--left`, `--right`, `--outer`, `--semi`
  (left rows with a match, no merge), `--anti` (left rows without a match).
- Field collisions: right side wins today. Add `--prefix-right r_` or
  `--into d.right` (nest the right record under a key) to avoid silent
  overwrites.
- Order check on both inputs, like `--by`; error names the file and line.

### Hash join (`--ref`) — S2

Today's mode, kept for the common case "small lookup table, big unsorted
stream":

```sh
cat big.ndjson | ndjson join --ref lookup.ndjson --on d.code
```

The reference file is loaded into a hash map keyed by the composite key; the
stream is not sorted and order is preserved. Supports `--left`, `--semi`,
`--anti`. `--ref` is the only way to get an in-memory join, so the cost is
always visible in the command line.

## Migration

| Today | After |
|-------|-------|
| `ndjson sort -f d.a -f d.b:d` | same flags (`-f` kept as alias of `-k`), external internally |
| `ndjson join --ref r --ref-fields a b --stream-fields c d` | `--ref r --left-on c,d --right-on a,b` (stdin is the left side, `--ref` the right) as a composite key; old flags accepted with a deprecation note **and the semantic change called out in the changelog** |
