# Plans: ndjson as an O(1)-space dataframe toolkit

`ndjson` and `ndjson-spatial` should behave like coreutils for JSON records:
small commands that read a stream on stdin, write a stream on stdout, and
compose with pipes (including with `sort`, `uniq`, `head`, `split`, etc.).
The defining constraint is **memory that does not grow with the input**.

## Documents

| File | What it covers |
|------|----------------|
| [01-commands.md](01-commands.md) | Full command inventory for both binaries, with space class and status |
| [02-select-and-expressions.md](02-select-and-expressions.md) | `select` (replaces `pick-field`) and the `rmath-ast` expression language |
| [03-grouping-and-aggregation.md](03-grouping-and-aggregation.md) | Streaming aggregates, sorted-run group-by, sketches (replaces `agg`) |
| [04-sort-and-join.md](04-sort-and-join.md) | External sort, merge join, bounded hash join (fixes current in-memory `sort`/`join`) |
| [05-spatial.md](05-spatial.md) | `ndjson-spatial` commands and spatial functions inside expressions |

## Principles

1. **One record in, zero or more records out.** Every command is a stream
   transformer over newline-delimited JSON. A record is never required to be
   an object unless the command needs it to be.
2. **Memory is O(1) in the number of records by default.** Memory may grow
   with the size of a single record, with a user-supplied bound (`-n 10`), or
   with a side input the user explicitly hands over (`--ref file`) — never
   silently with stdin.
3. **Order is a contract, not an accident.** Commands that need ordered input
   (group-by, merge join, uniq) assume it the way `uniq` and `join(1)` do, and
   detect violations instead of producing wrong answers (`--check-sorted` is on
   by default; violation is an error with the offending line number).
4. **Disk is the escape hatch for global operations.** Global sort spills to
   temp files with a fixed memory budget, like GNU `sort -S`.
5. **One expression language.** `filter`, `select`, `sort`, `join`, grouping
   keys and spatial functions all parse the same grammar (built on
   `rmath-ast`), so anything you can compute you can filter, sort, or group by.
6. **Errors are records too.** Bad input lines go to stderr with a line number;
   `--strict` makes them fatal. Default is skip-and-report, matching today.

## Space classes

Every command in [01-commands.md](01-commands.md) is tagged with one of:

| Class | Memory grows with | Examples |
|-------|-------------------|----------|
| **S0** | nothing (one record + fixed state) | `filter`, `select` scalars, `select count()`, sorted group-by, merge join |
| **S1** | a user-given bound *k* | `head -n`, `tail -n`, `sample -n`, `top -k` |
| **S2** | an explicit side input or the number of groups, opt-in | `join --ref` (hash), `select --by --hash`, spatial `--ref` R-tree |
| **Disk** | O(budget) memory, O(n) temp disk | external `sort` |
| **S∞** | the whole stream — **not allowed** | today's `sort`, `agg` |

`--help` for each command states its class.

## Current-state findings (from reading the code)

- ~~`sort` collects the whole stream (`sorted_by`) — S∞.~~ Done: `sort` is an
  external merge sort.
- `agg` sorts the whole stream to group it — S∞, and only one aggregate per run.
- `join` builds a map from the reference file per `--ref-fields` entry but
  reuses one reader, so only the first field sees data; and multiple field
  pairs are joined independently (OR) instead of as a composite key, which is
  not what the README's state+county example implies.
- `OrderedNumber` derives `Ord` on the enum, so every `PosInt` sorts before
  every `NegInt`, which sorts before every `Float` (`-1 > 5`, `2.5 > 3`).
  This affects `sort`, `agg`, and `join` keys.
- ~~`filter` uses a separate nom-4 grammar in `ndjson-common` that only supports
  `selector <op> literal`.~~ Done: `filter` is on rmath-ast.
- ~~`ndjson-spatial` is still on clap 2, geo 0.14, gdal 0.7.~~ Done: clap 4,
  geo 0.33, gdal 0.19.
- `rmath-ast` (now in `rmath-ast/` in this workspace) is parse-only
  (no evaluator), and its lexer has issues called out in
  [02](02-select-and-expressions.md#rmath-ast-changes).

## Roadmap

1. **Foundation** — fix `OrderedNumber` ordering; fix the rmath-ast lexer
   issues; add an evaluator. (Done: rmath-ast moved into this workspace and
   onto `yajlish-ndjson` 0.1 `Selector`.)
2. **`select` (scalar)** — projection, computed fields, aliases, `*`, `-r` raw
   output. Deprecate `pick-field` as an alias.
3. **`filter` on the new grammar** — boolean ops, keep old syntax working.
   (Done.)
4. **Streaming aggregates** — `select count(), sum(d.x)`; sorted `--by`;
   `--hash` opt-in. Deprecate `agg`. (Done, including `select` aggregate
   mode; sketches remain.)
5. **External sort + merge join** — replace `sort`/`join` internals; keep hash
   join behind `--ref`. (External sort done; merge join next.)
6. **Small S0/S1 commands** — `head`, `tail`, `sample`, `uniq`, `top`,
   `explode`, `flatten`, `to-csv`, `describe`.
7. **Spatial** — spatial functions in expressions; S0 geometry commands;
   clap 4 / geo upgrade. (Upgrades, `join-contains` and the functions done;
   shorthand commands, `explode-multi`, `validate` next.)
