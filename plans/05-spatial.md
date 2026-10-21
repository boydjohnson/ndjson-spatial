# ndjson-spatial

Same principles as `ndjson`: stream GeoJSON features one per line, O(1)
memory by default, and anything that needs a reference geometry set takes it
through an explicit `--ref` (held in an R-tree, S2).

## Spatial functions in the expression language

The main change: geometry operations become functions registered into the
shared registry (see [02](02-select-and-expressions.md#function-library)), so
they work in `select`, `filter`, `sort -k`, and `--by`. The `ndjson-spatial`
binary is `ndjson` plus the spatial function set (behind the `spatial` feature,
because of the GEOS/GDAL system dependencies).

```sh
# area and centroid as fields
ndjson-spatial select '*, area(d.geometry) as area, centroid(d.geometry) as c'

# filter by geometry
ndjson-spatial filter 'geom_type(d.geometry) == "Polygon" and area(d.geometry) > 1e6'

# spatial group-by via a cell key, O(1) memory
ndjson-spatial select '*, geohash(d.geometry, 6) as cell' \
  | ndjson sort -k d.cell \
  | ndjson select 'd.cell, count(), sum(d.properties.pop)' --by d.cell

# extent of the whole stream
ndjson-spatial select 'extent(d.geometry)'
```

### Scalar (S0)

| Function | Returns |
|----------|---------|
| `area(g)`, `length(g)` | planar; `geodesic_area`, `geodesic_length` for lon/lat |
| `centroid(g)`, `point_on_surface(g)` | Point geometry |
| `bbox(g)` | `[minx, miny, maxx, maxy]` |
| `x(p)`, `y(p)` | coordinates of a Point |
| `geom_type(g)`, `num_points(g)`, `is_valid(g)`, `is_empty(g)` | |
| `simplify(g, tol)`, `buffer(g, d)`, `convex_hull(g)`, `make_valid(g)` | geometry |
| `transform(g, "EPSG:4326", "EPSG:3857")` | geometry (existing `transform` command's logic) |
| `distance(g, wkt)`, `intersects(g, wkt)`, `contains(g, wkt)`, `within(g, wkt)` | against a literal geometry |
| `to_wkt(g)`, `from_wkt(s)` | |
| `geohash(g, precision)`, `quadkey(g, zoom)`, `h3(g, res)` | cell id of the centroid (or point) |

### Aggregate (S0)

| Function | State |
|----------|-------|
| `extent(g)` | running bbox |
| `centroid_agg(g)` | weighted running sum |
| `union_agg(g)` | **S2** — grows with geometry complexity; behind `--allow-unbounded` |

## Commands

| Command | Class | Status | Notes |
|---------|-------|--------|-------|
| `from-geojson`, `to-geojson` | S0 | exists | `from-geojson` already streams via yajlish. |
| `centroid`, `area`, `transform` | S0 | exists → shorthand | Become thin wrappers over `select '*, f(d.geometry) as …'`. Keep flags (`--field-name`, `--bbox`). |
| `length`, `bbox`, `simplify`, `buffer`, `convex-hull`, `make-valid` | S0 | new | Shorthands like the above; the real work is the function. |
| `explode-multi` | S0 | new | MultiPolygon → one feature per Polygon (like `ndjson explode`). |
| `validate` | S0 | new | Report invalid geometries with line numbers; `--fix` applies `make_valid`. |
| `join-contains --ref` | S2 | change | Point-in-polygon / containment join; R-tree over the ref file. Adopt `--on`-style options from [04](04-sort-and-join.md) (`--left`, `--semi`, `--anti`, `--into`). |
| `intersection --ref` | S2 | change | Same conventions; today only polygon/polygon. |
| `nearest-distance --ref` | S2 | change | Same conventions; today only point/point. Add `--k` nearest. |
| `tile --zoom z` | S0 | new (later) | Clip each feature to the tiles it touches; emits one record per (feature, tile). Pairs with `partition` for map-tile pipelines. |

### Making reference joins O(1)

For very large reference sets, a sorted-cell merge join avoids holding the
ref side in memory: assign both sides a cell key (`quadkey`/`h3`), externally
sort both by cell, then merge-join per cell and run the exact predicate within
the cell. Features spanning several cells are emitted once per cell and
de-duplicated by `uniq` on (left id, right id). This is the spatial analogue
of [04](04-sort-and-join.md)'s merge join; a later phase.

## Housekeeping

- clap 2 → 4 (same migration just done for `ndjson`).
- geo 0.14 → current, geojson 0.19 → current, geos 7 → current, gdal 0.7 →
  current. `geojson-rstar` is a git dependency; publish or replace with
  `rstar` + `geo` directly.
- Decide whether GDAL is still needed once `transform` uses `proj` directly;
  dropping it would make the spatial binary much easier to build.
