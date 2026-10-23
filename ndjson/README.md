<h1 align="center">ndjson</h1>

> Command line tool for creating, filtering, and processing ndjson

[![boydjohnson](https://circleci.com/gh/boydjohnson/ndjson-spatial/tree/master.svg?style=shield)](https://circleci.com/gh/boydjohnson/ndjson-spatial/tree/master)

### 🏠 [Homepage](https://github.com/boydjohnson/ndjson-spatial)

## Usage

```sh
SUBCOMMANDS:
    agg           Aggregatation commands on a grouped-by key
    filter        returns only json that matches filter expression
    from-json     Converts json to ndjson
    head          Outputs the first n records
    join          Joins two ndjson inputs on a key
    pick-field    picks a field from all of the ndjson objects
    tail          Outputs the last n records
```

## Use-case
### From-Json
Convert JSON to NDJSON

```sh
cat <file.json> | ndjson from-json <selector>
```

### Example

```sh
cat senators.json | ndjson from-json d.objects
```

### Filter

Filter NDJSON

```sh
cat <file.ndjson> | ndjson filter <selector>
```

### Example
Filter where field is equal to a value

```sh
cat <complaints.ndjson> | ndjson filter 'd.product == "Virtual Currency"'
```

Expressions can combine comparisons with `and`, `or`, and `not`, compute
values, test lists and nulls, and call functions:

```sh
ndjson filter 'd.price * d.qty > 100 and d.state in ["MN", "WI"]'
ndjson filter 'd.manager is not null or not d.active'
ndjson filter 'tolower(d.product) == "mortgage"'
```

A missing field is `null`. Records where the expression is `false` or
`null` are dropped; records where it can't be evaluated (e.g. `"abc" * 2`)
are dropped too and counted on stderr. The older unquoted form
`d.product == Virtual Currency` still works.

### Head and Tail

The first or last `n` records (default 10). Records pass through unchanged and
blank lines are skipped. `head` stops reading once it has `n` records, so the
upstream command exits early; `tail` keeps only the last `n` in memory.

```sh
cat <file.ndjson> | ndjson head -n 5
cat <file.ndjson> | ndjson tail -n 5
```

### Join

Join two ndjson inputs on a key. Keys are comma-separated expressions,
matched as a tuple and compared in the same order `ndjson sort` uses (so `1`
and `1.0` match). A key with a `null` (or missing field), array or object
part never matches. On field collisions the right record's value wins, unless
`--prefix-right r_` renames its fields or `--into d.right` nests the whole
right record.

By default `join` is a merge join: both inputs must be sorted ascending by
their keys, and memory holds only the right records of one key (at most
`--max-run`, default 1000000). A record out of order is an error that names
the input and line. Either input can be `-` for stdin.

```sh
ndjson sort -k d.state -k d.county < counties.ndjson > counties.sorted.ndjson
ndjson sort -k d.state -k d.county < population.ndjson > population.sorted.ndjson
ndjson join counties.sorted.ndjson population.sorted.ndjson --on 'd.state, d.county'
cat left.ndjson | ndjson join - right.ndjson --left-on d.st --right-on d.state
```

With `--ref` the reference file is loaded into memory as the right side and
the left input (stdin unless given) can be in any order, which it keeps:

```sh
cat big.ndjson | ndjson join --ref lookup.ndjson --on d.code
```

Join types: inner (default), `--left` (also unmatched left records),
`--right` and `--outer` (also unmatched right records, or both; merge join
only), `--semi` (left records with a match, once each) and `--anti` (left
records without a match). Unmatched, `--semi` and `--anti` records are
written unchanged.

`--ref-fields` and `--stream-fields` still work but are deprecated: they are
now `--right-on` and `--left-on`, and their fields are matched together as one
composite key rather than each pair on its own.

### Agg
Perform a group-by operation and then get Sum, Min, Max, or Count information.

```sh
cat <file.ndjson> | ndjson agg --group-by <selector> --agg <count | sum | min | max> <selector>
```

### Example
Get the sum of some numeric field grouped by another field.

```sh
cat state_county.ndjson | ndjson agg --group-by d.state --agg sum d.population 
```

### Pick-Field
Get a single value from all NDJSON objects

```sh
cat <file.ndjson> | ndjson pick-field <selector>
```

### Example

Use sort and uniq to get the count of distinct values of some field

```sh
cat senators.ndjson | ndjson pick-field d.person.gender | sort | uniq -c | sort -n
```

## Build

```sh
cargo build -Z unstable-options --profile=release-ndjson
```

## Run tests

```sh
cargo test
```

## Author

👤 **Boyd Johnson**

* Website: https://boydjohnson.dev
* Twitter: [@boydjohnson04](https://twitter.com/boydjohnson04)
* Github: [@boydjohnson](https://github.com/boydjohnson)

## 🤝 Contributing

Contributions, issues and feature requests are welcome!<br />Feel free to check [issues page](https://github.com/boydjohnson/ndjson-spatial/issues). 

## Show your support

Give a ⭐️ if this project helped you!

## 📝 License

<p>
  <a href="https://www.apache.org/licenses/LICENSE-2.0.txt" target="_blank">
    <img alt="License: Apache--2.0" src="https://img.shields.io/badge/License-Apache--2.0-yellow.svg" />
  </a>
</p>

Copyright © 2021 [Boyd Johnson](https://github.com/boydjohnson).<br />
This project is [Apache--2.0](https://www.apache.org/licenses/LICENSE-2.0.txt) licensed.

***
_This README was generated with ❤️ by [readme-md-generator](https://github.com/kefranabg/readme-md-generator)_