# Command inventory

Status: **exists** (keep as is), **change** (exists, behaviour/internals
change), **new**, **deprecate** (kept as an alias that prints a hint).
Space classes are defined in [README.md](README.md#space-classes).

## ndjson

### Input / output

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `from-json <selector>` | S0 | exists | Streams an array inside a JSON document via yajlish. |
| `from-csv [-d delim]` | S0 | change | Add `--no-header`, `--infer/--strings` (today infers types per cell). Make delimiter a flag, not a positional. |
| `to-csv [--columns a,b]` | S0 / S1 | new | With `--columns`, S0. Without, infer header from the first *k* records (`--infer-rows 100`, S1); keys first seen later are an error or dropped (`--extra=drop`). |
| `to-json` | S0 | new | Wrap the stream in `[ ... ]`. |
| `validate` | S0 | new | Exit non-zero on the first unparsable line; `--count` reports totals. |

### Row-wise (one record → 0..n records)

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `filter <expr>` | S0 | change | Moves to the shared expression grammar; gains `and`/`or`/`not`, computed comparisons (`d.a * 2 > d.b`), `in [..]`, `is null`. Old `d.x == Virtual Currency` (unquoted string) syntax keeps working. |
| `select <exprs>` | S0 | new | Projection + computed fields + aliases + `*`. See [02](02-select-and-expressions.md). |
| `pick-field <selector>` | S0 | deprecate | Alias for `select -r <selector>`. |
| `explode <selector>` | S0 | new | One output record per element of an array field (`unnest`). `--keep-empty` emits one record with null. |
| `flatten [--sep .]` | S0 | new | Nested objects → dotted keys; `unflatten` is the inverse. Useful before `to-csv`. |
| `uniq [-c] [--by expr]` | S0 | new | Drop *adjacent* duplicates, like `uniq(1)`. `-c` adds a count field. |

### Bounded-window

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `head [-n 10]` | S1 | new | Exits early so upstream gets SIGPIPE. |
| `tail [-n 10]` | S1 | new | Ring buffer of *n* records. |
| `skip -n N` | S0 | new | Offset. |
| `sample -n K` / `sample --rate p` | S1 / S0 | new | Reservoir sampling (Algorithm L) / Bernoulli. `--seed` for reproducibility. |
| `top -k K --by <expr> [--desc]` | S1 | new | Bounded heap; the O(k) answer to `sort | head`. With `--by-group`, see [03](03-grouping-and-aggregation.md). |

### Aggregation

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `select count(), sum(d.x), ...` | S0 | new | Whole-stream aggregates with constant state. |
| `select ... --by <expr>` | S0 | new | Group-by over **sorted** input (adjacent runs). Errors on out-of-order keys. |
| `select ... --by <expr> --hash` | S2 | new | One accumulator set per distinct key; for unsorted input with few groups. |
| `agg --group-by g --agg f sel` | — | deprecate | Rewrites to `select <f>(sel) --by g --hash`. |
| `describe` | S0 (per field) | new | Per-field count, null count, type histogram, min, max, mean, stddev, approx distinct (HLL), approx p50/p90/p99 (KLL/t-digest). Memory grows with the number of distinct *field names*, not records. |

### Ordering and combining

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `sort -k <expr>[:d] ... [-S 256M]` | Disk | change | External merge sort with a memory budget and temp-file spill; stable; `--check` only verifies order. See [04](04-sort-and-join.md). |
| `join <left> <right> --on <expr>[,<expr>]` | S0 | change | Merge join over two sorted streams (one may be stdin). Composite keys. `--left/--outer/--anti/--semi`. |
| `join --ref <file> --on ...` | S2 | change | Hash join, reference file held in memory (today's behaviour, fixed for composite keys). |
| `partition --by <expr> --out 'dir/{key}.ndjson'` | S0 (+ open files) | new | Route records to files by key; caps open handles with an LRU (`--max-open 256`). The divide step for divide-and-conquer on big data. |

### Misc

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `ndjson-dataset iris` | — | exists | Example data. |

## ndjson-spatial

Summary only; details in [05-spatial.md](05-spatial.md).

| Command | Class | Status |
|---------|-------|--------|
| `from-geojson`, `to-geojson` | S0 | exists |
| `centroid`, `area`, `transform` | S0 | exists → fold into expression functions, keep commands as shorthands |
| `length`, `bbox`, `simplify`, `buffer`, `convex-hull`, `make-valid`, `explode-multi` | S0 | new |
| `extent` | S0 | new (aggregate) |
| `geohash` / `quadkey` / `h3` cell assignment | S0 | new (as functions) |
| `intersection --ref`, `join-contains --ref`, `nearest-distance --ref` | S2 (ref in R-tree) | exists → change to the shared `--ref`/`--on` conventions |

## Naming conventions

- Expressions are positional when a command takes exactly one; otherwise flags
  (`--by`, `--on`, `-k`).
- `-r/--raw` means "emit bare JSON values instead of objects" wherever it
  applies.
- `--ref <file>` always means "this side input is loaded into memory" (S2).
