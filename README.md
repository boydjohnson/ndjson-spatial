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
    ndjson pick-field <expression>

    ndjson pick-field "d.properties.distance "
```

```bash
    ndjson join --ref <ref> --ref-field <ref-fields> --stream-field <stream-field>
```
    - ref-fields, stream-fields should be space delimited list of 'd.properties.FIELD_NAME'

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
