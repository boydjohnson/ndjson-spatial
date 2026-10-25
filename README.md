This repo contains some Rust cli tools for working with new-line delimited geojson

These tools are currently pre-release. There will be releases with builds for linux and macos.

# Build

This repo requires a nightly compiler before 2020-09-28 exclusive.

To build
```
rustup default nightly-2020-09-27
cargo build --release
```

for lto (optimizations for code speed and size) on `ndjson`

```
cd ndjson
cargo build -Z unstable-options --profile=release-ndjson

ls -lh ../target/release-ndjson/ndjson
```

## Current

## ndjson-spatial

```bash
    ndjson-spatial centroid
````

```bash
    ndjson-spatial intersection --ref <reference-file.geo.json>
```
    - currently: polygon/polygon

```bash
    ndjson-spatial nearest-distance --ref <ref> --field-name
```
    - currently: point/point

```bash
    ndjson-spatial area --field-name <field-name>
```
    - square meters from lon/lat (geodesic); `--planar` for projected coordinates

```bash
    ndjson-spatial from-geojson

    ndjson-spatial to-geojson
```

## ndjson

```bash
    ndjson filter <expression>

    ndjson filter "d.properties.Area >= 30000.0"
```

```bash
    ndjson select <list> [-r | --tsv]

    ndjson select 'd.name, d.price * d.qty as total, toupper(d.state) as state'
    ndjson select '*, d.price * d.qty as total'
    ndjson select '* except (d.ssn, d.internal)'
    ndjson select -r 'd.person.gender' | sort | uniq -c
```

```bash
    ndjson head -n 10 | ndjson tail -n 10 | ndjson skip -n 100
    ndjson sample -n 1000 [--seed 7]        # exactly 1000 records, uniformly
    ndjson sample --rate 0.01               # each record with probability 1%
    ndjson top -k 5 --by d.pop:d            # same as sort -k d.pop:d | head -n 5, in O(k) memory
    ndjson sort -k d.state | ndjson uniq -c --by d.state
    ndjson explode d.tags                   # one record per element of d.tags
    ndjson flatten [--sep .] | ndjson unflatten
    ndjson to-json                          # one JSON array
    ndjson validate [--count]
```

```bash
    ndjson pick-field <expression>

    ndjson pick-field "d.properties.distance "
```

```bash
    ndjson join <left> <right> --on <keys>
```
    - keys are comma-separated expressions like 'd.properties.FIELD_NAME'; both inputs must be sorted by them, or use --ref <file> to load the right side into memory

## Proposed

## ndjson-spatial

```bash
    ndjson-spatial join-contains --ref <reference-file.geo.json> --field-name <field-name>
```

## ndjson

```bash
    ndjson select-count <expression> --selector <selector> --field-name <field-name>

    ndjson select-count d.properties.join --selector light=yes --field-name NUM_W_LIGHT
```

```bash
    ndjson select-avg <expression> --selector <selector> --field-name <field-name>

    ndjson select-avg d.properties.join --selector light=yes --field-name AVG_W_LIGHT
```
