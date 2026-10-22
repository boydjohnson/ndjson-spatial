# Grouping and aggregation

Replaces `agg`. Aggregation is expressed in `select` (see
[02](02-select-and-expressions.md)); this document covers how grouping runs.

**Status:** the three strategies below are implemented in
`ndjson/src/group.rs` over `rmath_ast::AggregateList`. `select` uses them
in aggregate mode (`--by`, `--grouped`, `--hash`, `--max-groups`,
`--allow-unbounded`), and so does `agg` (hash by default, `--sorted`).
Hash groups live in a `BTreeMap`, so output is always in key order and
`--unordered` isn't needed. Still to do: the sketches, `scan`, and top-k per
group.

## Three execution strategies

### 1. No grouping — S0

```sh
ndjson select 'count(), sum(d.amount), approx_quantile(d.amount, 0.9) as p90'
```

One accumulator set, one output record at EOF. Empty input emits one record
(`count` = 0, others null), matching SQL.

### 2. Sorted runs (`--by`) — S0, the default

```sh
ndjson sort -k d.state | ndjson select 'd.state, count(), sum(d.pop)' --by d.state
```

Like `uniq -c`: keep one accumulator set for the *current* key; when the key
changes, emit and reset. Memory is constant regardless of the number of groups.

- Input must be grouped (contiguous), not necessarily sorted. Detecting a key
  that reappears after its run ended needs memory, so the check is:
  - default: verify keys are non-decreasing (cheap; catches unsorted input
    and gives a clear error: `line 8812: key "IA" < previous "MN"; input
    must be sorted by --by, or use --hash`).
  - `--grouped`: skip the order check, trust contiguity (for streams grouped
    but not sorted, e.g. by time window).
- Multiple keys: `--by d.state,d.county` (composite, compared lexicographically).
- Output is in key order, so it composes with merge join and further
  `--by` steps.

### 3. Hash (`--by ... --hash`) — S2, opt-in

One accumulator set per distinct key in a hash map; emit all at EOF (sorted by
key for deterministic output, `--unordered` to skip). For unsorted input with
a small known number of groups. `--max-groups N` (default 1,000,000) aborts
with a clear error rather than eating all memory.

`agg --group-by g --agg f sel` maps to this strategy for backwards
compatibility (today's `agg` sorts everything in memory, which is strictly
worse).

## Sketches for "distinct" and "quantile"

Exact distinct counts and exact quantiles need memory proportional to the
data. The O(1) versions:

| Need | Function | Structure | Memory | Error |
|------|----------|-----------|--------|-------|
| distinct count | `approx_count_distinct(x[, p])` | HyperLogLog++ | 2^p bytes (p=14 → 16 KiB) | ~0.8% |
| quantiles | `approx_quantile(x, q)`, `median` | KLL | ~few KiB | ~1% rank error |
| heavy hitters | `approx_top_k(x, k)` | Space-Saving | O(k) | bounded |

Candidate crates: `hyperloglogplus`, `pdatastructs`, or small in-repo
implementations (both HLL and KLL are a few hundred lines). Sketches implement
`merge`, so per-partition results can be combined.

Exact versions (`count_distinct`, `collect`) exist but require
`--allow-unbounded`.

## Running / window aggregates (later)

`scan` emits every input record with running accumulators attached:

```sh
ndjson scan 'sum(d.amount) as running_total, count() as row_number' [--by d.account]
```

S0 with sorted `--by`. Lag/lead over a fixed offset (`lag(d.x, 1)`) is S1
(ring buffer of *k*). Arbitrary windows are out of scope.

## Top-k per group

`top -k 3 --by d.score --group d.state` — S0 with sorted groups (bounded heap
per current group), S2 with `--hash`.

## Divide and conquer

For "group by a high-cardinality key on unsorted data larger than RAM", the
answer is either `sort` (external, [04](04-sort-and-join.md)) then `--by`, or:

```sh
ndjson partition --by 'd.user_id % 64' --out 'parts/{key}.ndjson'
for f in parts/*.ndjson; do ndjson select 'd.user_id, count()' --by d.user_id --hash < "$f"; done
```

Each partition's group count is ~1/64 of the total.
