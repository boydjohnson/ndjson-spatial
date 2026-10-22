# `select` and the expression language

`select` replaces `pick-field`. It takes a comma-separated list of
expressions, evaluates each one against every record, and writes one output
record per input record (scalar mode) or one record per group / stream
(aggregate mode). The expression grammar comes from the sibling `rmath-ast`
crate and is shared with `filter`, `sort -k`, `join --on`, and `--by`.

## Examples

```sh
# projection (pick-field replacement, but many fields)
ndjson select 'd.name, d.address.city'
# {"name":"Ada","city":"London"}

# computed fields with aliases
ndjson select 'd.name, d.price * d.qty as total, toupper(d.state) as state'

# keep the whole record, add a field
ndjson select '*, d.price * d.qty as total'

# keep everything except some fields
ndjson select '* except (d.ssn, d.internal)'

# raw values for classic unix pipelines (what pick-field did)
ndjson select -r 'd.person.gender' | sort | uniq -c | sort -n

# whole-stream aggregates: one output record, O(1) memory
ndjson select 'count() as n, sum(d.amount), max(d.amount)'
# {"n":1042,"amount_sum":88123.5,"amount_max":991.0}

# the motivating example
ndjson select 'max(d.amount), d.name'
# {"amount_max":991.0,"name":"Grace"}   <- name from the row that held the max

# grouped aggregates over sorted input, O(1) memory
ndjson sort -k d.state | ndjson select 'd.state, count(), avg(d.salary)' --by d.state
```

## Grammar (additions on top of rmath-ast)

```
select_list := item ("," item)*
item        := "*" ["except" "(" selector ("," selector)* ")"]
             | expr ["as" ident]

expr        := or_expr
or_expr     := and_expr ("or" and_expr)*
and_expr    := not_expr ("and" not_expr)*
not_expr    := ["not"] cmp_expr
cmp_expr    := add_expr [cmp_op add_expr]
             | add_expr "is" ["not"] "null"
             | add_expr ["not"] "in" "[" literal ("," literal)* "]"
cmp_op      := "==" | "!=" | "<" | "<=" | ">" | ">="
add_expr    := (existing rmath-ast tiers: + - / * / ** )
unary       := "-" unary | term
term        := literal | selector | "(" expr ")" | call
call        := ident "(" [expr ("," expr)*] ")"
selector    := "d" ( "." ident | "." quoted | "[" int "]" | "[" quoted "]" )*
```

Precedence low → high: `or`, `and`, `not`, comparisons, `+ -`, `* /`, `**`,
unary `-`, call/selector.

## Output naming

When an item has no `as` alias:

| Item | Output key | Rule |
|------|-----------|------|
| `d.address.city` | `city` | last identifier of the selector |
| `d.tags[0]` | `tags_0` | last identifier + index |
| `max(d.amount)` | `amount_max` | `<selector-name>_<fn>` — matches today's `agg` naming |
| `count()` | `count` | |
| anything else | the expression text, whitespace-normalised | e.g. `"d.price * d.qty"`; a warning suggests `as` |

Two items producing the same key is a parse-time error.

## Output modes

- Default: one JSON object per output row, keys in select-list order.
- `-r/--raw`: one item → the bare JSON value per line (strings stay quoted,
  so the output is still valid ndjson). Several items → a JSON array per line.
- `--tsv`: several items → tab-separated plain text (strings unquoted,
  null as empty). For handing off to `sort -t$'\t'`, `cut`, `awk`.

## Scalar vs aggregate mode

A select list is in **aggregate mode** if any item contains an aggregate call.
In aggregate mode output is emitted at end-of-stream (or at each group
boundary with `--by`).

Mixing aggregate and bare (non-aggregate) items, as in
`select max(d.amount), d.name`:

1. **Recommended — "arg" semantics (SQLite bare-column rule).** If the list
   contains exactly one `min()` or `max()`, bare items are evaluated on the
   record that produced that extreme. This is O(1): keep the current best
   record's bare values alongside the accumulator. Ties keep the first record
   (or `--ties last`).
2. With `--by`, bare items that are textually equal to a `--by` key are
   allowed (they're constant within the group).
3. Anything else (bare item next to `sum`, `count`, or two `max`es) is a
   parse-time error that suggests `first(d.name)`, `last(d.name)`,
   `argmax(d.amount, d.name)`, or adding `--by d.name`.

We do **not** implicitly group by bare columns: that would silently make the
command S2 or require sorted input the user didn't ask for.

## Function library

All functions go through one registry (`name → kind, arity, impl`) so
`ndjson-spatial` can register geometry functions into the same language.

**Scalar** (S0, per record)

- math: `abs sqrt floor ceil trunc round(x[,n]) exp log log10 sin cos tan …`
  (the existing rmath-ast unary set), `min2(a,b)`, `max2(a,b)` (or `least`/`greatest`, to avoid clashing with aggregates)
- strings: `toupper tolower capitalize len substr(s,i[,n]) concat(...)
  split(s,sep) replace(s,from,to) trim starts_with ends_with contains
  regex_match(s,re) regex_extract(s,re[,group])`
- types: `tostring tofloat toint tobool typeof isnull isfloat isfloatnan`
- control: `coalesce(...)`, `if(cond, a, b)`
- json: `keys(obj)`, `len(array)`, `get(obj, "dyn-key")`

**Aggregate** (constant state unless noted)

| Function | State | Notes |
|----------|-------|-------|
| `count()`, `count(x)` | O(1) | `count(x)` counts non-null |
| `sum avg min max` | O(1) | numeric; non-numeric skipped with a warning count |
| `var stddev` (pop/sample) | O(1) | Welford |
| `first last any` | O(1) | |
| `argmin(key, val)`, `argmax(key, val)` | O(1) | explicit form of the bare-column rule |
| `approx_count_distinct(x)` | O(1) | HyperLogLog, ~1–2% error, `precision` arg |
| `approx_quantile(x, p)`, `median(x)` | O(1) | KLL sketch; `median` = p=0.5 |
| `count_distinct(x)` | **S2** | exact; grows with distinct values; requires `--allow-unbounded` |
| `collect(x)` | **S2** | builds an array; same flag |

Aggregates may wrap scalar expressions (`sum(d.price * d.qty)`) and be wrapped
by them (`round(avg(d.x), 2)`); nested aggregates are a parse error.

Accumulators implement `update(&Value)`, `merge(other)`, `finish() -> Value`.
`merge` is unused at first but enables parallel/partitioned execution later.

## Evaluation semantics

- Missing path → `null`. `--strict` makes it an error with the line number.
- Arithmetic on `null` → `null`. Type errors (`"abc" * 2`) → `null`, counted,
  and summarised on stderr at exit (`select: 37 type errors in 'd.a * 2'`).
- Integers stay integers for `+ - *` (checked; overflow promotes to float);
  `/` always yields float; `**` yields float unless both sides are
  non-negative ints and the result fits.
- Comparisons coerce int ↔ float numerically. Comparing different JSON types
  (string vs number) is false for `<`/`>` and `!=` is true; `sort` uses a
  total order defined in [04](04-sort-and-join.md#value-ordering).

## rmath-ast changes

Findings from reading `rmath-ast` (`src/math.lalrpop`, `src/parser/ast.rs`):

1. **Selector type.** It uses `yajlish::ndjson_handler::Selector` from a git
   dependency with the old `ndjson` feature. Switch to
   `yajlish_ndjson::Selector` from crates.io 0.1 to match `ndjson-common`.
2. **Identifier lexing.** `IdentifierToken` is `\.[^\[\./\*\+\^ -]+`, which
   swallows `,` `(` `)` `=` `<` `>` `!` `"`. So `max(d.amount)` lexes the
   identifier as `amount)`, and `d.a, d.b` as `a,`. Restrict to
   `[A-Za-z_][A-Za-z0-9_]*` and add quoted forms `."key with spaces"` and
   `["key"]`.
3. **String lexing.** `StringP` is `".*"` (greedy), so `"a" + "b"` is one
   token. Use `"([^"\\]|\\.)*"` and unescape.
4. **Negative literals in the lexer.** `Int` is `[-][0-9]+`, so `3-5` lexes as
   `3`, `-5` and fails to parse. Drop the sign from the token and add unary
   minus to the grammar.
5. **Float lexing.** Doesn't accept `1.5e3`, `.5`, or `1E5`.
6. **Functions.** ~~`UnaryFunction` is a closed, single-argument enum matched
   by one regex, so nothing outside the crate can add functions.~~ Done:
   calls parse to `Expr::Call(String, Vec<Expr>)` with any number of
   arguments, and `FunctionRegistry::resolve`/`compile` binds them (checking
   arity) into a `CompiledExpr` for evaluation. The existing functions are
   registered as built-ins (`rmath-ast/src/builtins.rs`); other crates add
   theirs with `FunctionRegistry::register`. Aggregates are a second
   `FunctionKind` (`rmath-ast/src/aggregate.rs`): `count sum avg min max
   first last argmin argmax var var_pop stddev stddev_pop`, plus unbounded
   `count_distinct collect`. `AggregateList::compile` splits a list into
   accumulator slots and finishing expressions and applies the bare-column
   rules below; plain `resolve` rejects aggregate calls. Sketches
   (`approx_count_distinct`, `approx_quantile`, `median`, `approx_top_k`) are
   still to do.
7. **Missing pieces.** ~~comparisons, boolean ops, `in`, `is null`, and an
   evaluator~~ (done: `Expr::Compare/And/Or/Not/IsNull/In`, evaluated with
   three-valued `null` logic and short-circuiting; `CompiledExpr::eval_bool`
   for predicates; `filter` uses it). ~~Still missing: select list, `as`,
   `*`/`except`.~~ Done: the `SelectList` rule parses to `SelectItem`s
   (`Star { except }` or `Expr { expr, alias, span }`), and
   `ndjson/src/select.rs` names, resolves and evaluates them.
8. **Toolchain.** ~~lalrpop 0.19 → current; nom 7 is listed but unused by the
   grammar.~~ Done: lalrpop 0.23, and the unused nom dependency is dropped.

### Where rmath-ast lives

**Decided: in this workspace** at `rmath-ast/`, imported with
`git subtree add` so its history is kept. ndjson and ndjson-spatial won't be
published to crates.io, so only development ergonomics matter, and keeping the
grammar next to the commands that use it is simplest. It is now a workspace
member and uses `yajlish_ndjson::Selector` (item 1 above is done). It may be
renamed (e.g. `ndjson-expr`) once it grows past arithmetic.

## Performance notes (later)

- Today each line is fully parsed into a `serde_json::Value`. When the select
  list touches only a few paths of a large record (GeoJSON geometries), a
  yajlish handler that materialises only the referenced paths avoids the
  allocation. `select *` still needs the full parse.
- Compile the expression once to a small tree of closures / bytecode instead
  of re-walking the AST per record.
