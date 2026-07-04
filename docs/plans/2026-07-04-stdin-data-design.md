# stdin data / spec separation — design

**Date:** 2026-07-04
**Status:** approved
**Scope:** data arrives on stdin separately from the spec; columnar data form;
declared column types. Temporal scales are explicitly OUT — next cycle
(declared date types map to ordinal as documented interim behavior).

## Why

benday exists to pair with query engines: results piped to charts. Today the
spec and its data are one document, so every caller must embed rows inside the
spec JSON — boilerplate, and a place for transposition mistakes benday cannot
detect. The natural agent flow is:

```
query ... | benday --spec '{"mark":"bar", ...}'
```

The concrete producer is Filip's internal tooling (mcp-bigquery today,
mcp-dataconnector next), which emits a **columnar envelope** as MCP
`structuredContent`:

```json
{
  "columns": [ {"name": "day", "type": "STRING"}, {"name": "n", "type": "INT64"} ],
  "rows":    [ ["mon", 32], ["tue", 78] ],
  "total_rows": 123,
  "truncated": true,
  "query": { "job_id": "...", "...": "provenance benday ignores" }
}
```

Accepting this form natively means near-zero reshaping for the agent, and the
declared `columns[].type` beats type inference exactly in the annoying cases
(numeric-looking codes, dates as strings).

## Interface semantics

**stdin's role is determined by where the spec came from.** No mode flags:

- Spec via `--spec`/`--spec-file` → stdin is **data**.
- No spec flag → stdin is the **spec** (today's behavior, unchanged).
- Fully inline spec with inline data, no stdin: unchanged.

**The data document on stdin** is auto-detected by shape; two forms:

1. **Columnar envelope**: `{"columns": [...], "rows": [[...]]}` plus known
   envelope fields `truncated` and `total_rows` (flow through to `--meta`).
   Unknown envelope keys (`query`, …) are **ignored**: the data document is
   producer-shaped payload, unlike the agent-authored spec where an unknown
   key means a misunderstanding. Pipe `structuredContent` straight in.
2. **Bare JSON array of objects**: `[{"col": val}, ...]`.

NDJSON: deferred until a real producer needs it.

**Precedence is strict, never silent:**

- Spec has inline data AND stdin has data → error "data provided twice", exit 2.
- Spec has no data and stdin is a TTY/empty → error naming both accepted forms.
- Data piped but `--spec` forgotten (stdin parses as a spec but looks like a
  data document — has `columns`/`rows`, or is an array) → targeted error:
  "this looks like data; pass the spec via --spec".

**Structural strictness inside the data:** a row whose length doesn't match
`columns` is a hard error (row index + both lengths in the message), not a
dropped row — length mismatch means transposition corruption, the exact
failure an agent reading dot art can't see. `dropped_rows` stays reserved for
value-level problems (nulls, unparseable numbers), as today.

## Spec grammar and declared types

`spec.data` becomes **optional** and gains the columnar form:

```jsonc
"data": { "values": [ {...}, ... ] }              // unchanged
"data": { "columns": [...], "rows": [[...]] }     // columnar, new
// "data" omitted → rows must arrive on stdin
```

Inline columnar is needed by the corpus (spec→scene tests cover columnar
ingestion without CLI plumbing). `deny_unknown_fields` still applies to the
spec's data object — the tolerant envelope applies only to the stdin document.

**Declared type mapping** (case-insensitive, BigQuery + common SQL spellings):

- `INT64` `INTEGER` `FLOAT64` `FLOAT` `DOUBLE` `NUMERIC` `BIGNUMERIC` `DECIMAL`
  → quantitative
- `DATE` `DATETIME` `TIMESTAMP` `TIME` → **ordinal this cycle** (ISO strings
  sort lexically = chronologically; documented interim → temporal next cycle).
  To make that actually hold for shuffled input, ordinal x categories are
  SORTED lexically at compile time (nominal keeps first-seen order, unchanged)
- `STRING`, `BOOL`, anything unrecognized → nominal. Unknown type names are
  NOT an error: producers grow types; nominal fallback is safe-wrong-in-the-
  obvious-way, not silent-wrong.

**Type resolution precedence:** explicit spec `"type"` > declared column type
> inference from data. The spec is the agent's stated intent and always wins.

**Scope:** the precedence chain applies where inference happens today — the
x/y channels of line/point/area (plus the ordinal-sort decision for bar x).
Bar y is NOT type-gated: it has always coerced values numerically row-by-row
(`num()`: numbers, numeric strings, bools), with non-coercible values counted
in `dropped_rows`. A declared `STRING` y column whose values parse as numbers
still charts as a bar — that is today's documented coercion contract, not an
inference bug for declared types to override. Tightening bar-y typing is out
of scope for this cycle.

`--meta` grows a `data` block — `{source, truncated, total_rows}` —
**conditionally**: only when data came from stdin, or the envelope reported
`truncated`/`total_rows`. Meta reports what the caller can't already know;
inline data is the caller's own bytes, so inline-values charts emit no data
block. (This also keeps the glyph-gallery bundles, which include meta,
byte-identical through this cycle — the referee stays intact.)

## Architecture

A new compile-side stage, `ingest`, in front of the existing pipeline:

```
        CLI                          benday-core
  stdin ──▶ DataDoc ─┐
                     ├─ ingest ──▶ Table ──▶ compile ──▶ Scene ──▶ rasterize
  --spec ──▶ Spec ───┘   (resolve precedence,
                          columnar→rows,
                          declared types)
```

- **`ingest` module** (zero I/O): parses a data-document string into `DataDoc`
  (tolerant-envelope serde type) and resolves spec + optional stdin doc into:

```rust
pub struct Table {
    pub rows: Vec<Map<String, Value>>,        // normalized row-major, as today
    pub declared: HashMap<String, FieldType>, // from columns[].type, mapped
    pub provenance: DataProvenance,           // source form, truncated?, total_rows?
}
```

Columnar input is zipped into row objects once, at ingest; everything
downstream keeps operating on the row-major form it already knows. No dual
code paths in the compiler.

- **`compile` grows one parameter**: `compile(spec, &table, &opts)`. Type
  resolution changes exactly one line: spec-type → `table.declared` →
  `infer_type`.
- **CLI stays a thin shell**: reads stdin when the spec came via flag, calls
  `ingest::parse_data_doc` then `ingest::resolve`. All precedence and
  data-shape errors live in core where the corpus can pin them.
- **Scene** carries the provenance in its `source` block — `--meta`'s data
  section and `--dump-scene` get it for free.

Compiler-axis work only; the rasterizer is untouched.

## Errors

Existing kinds, no new ones:

- Call-shape mistakes → `spec`, exit 2: data twice; no data anywhere;
  stdin-looks-like-data redirect.
- Payload problems → `data`, exit 3: malformed stdin JSON, row/columns length
  mismatch, empty `rows`.

## Testing

- **Corpus** (the bulk): columnar inline data; declared-type resolution
  (declared beats inference, spec beats declared); DATE→ordinal interim;
  unknown type→nominal; length-mismatch error; empty-rows error; `resolve`
  precedence errors (ingest is pure — error text snapshot-pinned like compile
  errors).
- **CLI integration tests** (new): stdin routing is CLI behavior. `assert_cmd`
  dev-dependency in benday-cli, ~6 tests: pipe data + `--spec` renders; pipe
  spec alone still works; data-twice fails exit 2; forgotten `--spec` gets the
  redirect error; `--meta` shows `truncated`; envelope with `query` block
  accepted. Closes the CLI-has-zero-tests gap for the paths this cycle
  touches.
- **Gallery**: one new snapshot from columnar data, mostly as documentation.

## Milestones

Net first, then build:

1. `DataDoc` + `Table` + `ingest::resolve`, corpus-style unit coverage,
   nothing wired.
2. `compile` takes `&Table`; existing corpus snapshots MUST NOT diff
   (values-form specs produce identical Scenes — the referee rule).
3. Declared-type resolution + new corpus cases.
4. CLI stdin routing + `assert_cmd` tests + `--meta` data block.
5. README + `examples/` update (a pipe example showing the envelope flow).

## Out of scope (deliberately)

Temporal scales (next cycle, with real piped data in hand). NDJSON. CSV.
`--data-file` flag. Any change to marks, scales, themes, or the rasterizer.
