# stdin Data / Spec Separation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Data arrives on stdin separately from the spec (`query ... | benday --spec '...'`),
in columnar-envelope or row-array form, with declared column types feeding type
resolution.

**Architecture:** New pure `ingest` module resolves spec + optional stdin data
document into a `Table` (row-major rows + declared types + provenance);
`compile()` gains a `&Table` parameter. CLI routes stdin by whether the spec
came via flag. Design doc: `docs/plans/2026-07-04-stdin-data-design.md` — read
it first. Rasterizer untouched.

**Tech Stack:** Rust, serde/serde_json, insta, assert_cmd (new, CLI tests).

**Referee rules for this cycle** (refines the foundation's rule):

- Glyph gallery TEXT must never diff. Gallery bundles include `--meta`; meta
  may not diff either (the new meta `data` block is conditional, see Task 4).
- Corpus snapshots may diff ONLY where a task explicitly authorizes it, each
  diff reviewed line-by-line: Task 2 authorizes exactly one (`err_empty_data`
  message), Task 4 authorizes the mechanical `source` additions.
- Anything else diffing = the code is wrong. Never blanket-accept.

**Execution notes:** same protocol as the foundation — one Opus subagent per
task, adversarial-review subagent after each, orchestrator runs `make validate`
and inspects diffs personally. Tasks are strictly sequential. Commit per task.

---

## Task 1: The `ingest` module (nothing wired)

**Files:**
- Create: `crates/benday-core/src/ingest.rs`
- Modify: `crates/benday-core/src/lib.rs` (add `pub mod ingest;`)

**Step 1: Write the types and public functions**

```rust
//! Data ingestion: resolve the spec's inline data and/or a piped data
//! document into a normalized `Table`. Pure — the CLI does the I/O.
//!
//! Strictness boundary: the SPEC is agent-authored intent, so its data object
//! is strict (`deny_unknown_fields`, over in `spec.rs`). The stdin document is
//! producer-shaped payload (e.g. an MCP `structuredContent` envelope), so it
//! is tolerant: known fields are used, unknown fields (query provenance etc.)
//! are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Error;
use crate::spec::{FieldType, Spec};

pub type Row = Map<String, Value>;

/// A data document piped to stdin: a columnar envelope or a bare row array.
/// Tolerant by design — no `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DataDoc {
    Envelope {
        columns: Vec<EnvColumn>,
        rows: Vec<Vec<Value>>,
        #[serde(default)]
        truncated: Option<bool>,
        #[serde(default)]
        total_rows: Option<u64>,
    },
    Rows(Vec<Row>),
}

/// Envelope column: tolerant twin of `spec::Column` (producers may add keys).
#[derive(Debug, Deserialize)]
pub struct EnvColumn {
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    InlineValues,
    InlineColumns,
    StdinValues,
    StdinColumns,
}

#[derive(Debug, Serialize)]
pub struct DataProvenance {
    pub source: DataSource,
    pub truncated: Option<bool>,
    pub total_rows: Option<u64>,
}

/// Normalized data ready for the compiler: row-major rows (as the compiler
/// has always consumed), declared column types, and where it all came from.
pub struct Table {
    pub rows: Vec<Row>,
    pub declared: HashMap<String, FieldType>,
    pub provenance: DataProvenance,
}

/// Parse a stdin data document. Wraps serde's unhelpful untagged-enum error
/// with the two accepted shapes.
pub fn parse_data_doc(s: &str) -> Result<DataDoc, Error> {
    serde_json::from_str(s).map_err(|e| {
        Error::Data(format!(
            "cannot parse stdin as a data document; expected \
             {{\"columns\":[{{\"name\",\"type\"?}}...],\"rows\":[[...]...]}} or a JSON array \
             of row objects: {e}"
        ))
    })
}

/// Map a declared column type (BigQuery + common SQL spellings, case-
/// insensitive) to a field type. Unknown names fall back to nominal — NOT an
/// error: producers grow types, and nominal is safe-wrong-in-the-obvious-way.
/// DATE/TIMESTAMP map to ordinal THIS CYCLE (ISO strings sort lexically =
/// chronologically); they become temporal in the next cycle.
pub fn declared_field_type(t: &str) -> FieldType {
    match t.to_ascii_uppercase().as_str() {
        "INT64" | "INTEGER" | "INT" | "SMALLINT" | "BIGINT" | "FLOAT64" | "FLOAT"
        | "DOUBLE" | "NUMERIC" | "BIGNUMERIC" | "DECIMAL" | "REAL" => FieldType::Quantitative,
        "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" => FieldType::Ordinal,
        _ => FieldType::Nominal,
    }
}

/// Resolve spec + optional stdin document into a Table. Owns ALL precedence
/// and data-shape errors so the corpus can pin them.
pub fn resolve(spec: &Spec, stdin: Option<DataDoc>) -> Result<Table, Error> {
    // ... see steps below; signatures and error TEXT are load-bearing.
}
```

`resolve` rules (implement exactly; every message below is pinned by tests):

1. Spec has data AND stdin is `Some` → `Error::Spec("data provided twice: the spec has inline `data` and a data document arrived on stdin; remove one")`.
2. Neither → `Error::Spec("no data: the spec has no `data` and nothing arrived on stdin; add data.values or data.columns+rows to the spec, or pipe a data document")`.
3. Columnar → rows zip (shared by inline and stdin forms):
   - duplicate column name → `Error::Data("duplicate column name \"{name}\"")`
   - row length ≠ columns length → `Error::Data("row {i} has {got} values but {want} columns are declared")` (i is 0-based)
   - build one `Row` per rows entry, keys in column order.
4. Declared types: for each column with a `type`, insert `declared_field_type`.
   `values`/`Rows` forms have an empty `declared` map.
5. Empty rows (any form) → `Error::Data("data has no rows; provide at least one row")`.
   (This check MOVES here from `preflight` — see Task 2.)
6. Provenance: source per the four variants; `truncated`/`total_rows` only from
   the envelope, `None` otherwise.

NOTE: in this task, `Spec::data` is still the old required-`values` struct;
`resolve` only handles `spec.data.values` + the stdin doc. The spec grammar
grows in Task 3. Until then rules 1–2 read "spec has data" as "always true",
so only the stdin=Some arm of rule 1 is reachable — that's fine; write the
match so Task 3 only changes the spec-side pattern.

**Step 2: Unit tests in the module** (corpus-style: insta inline snapshots for
error text, plain asserts for structure)

Cover: bare-array parse; envelope parse with extra `query` key (must succeed —
the tolerance test); envelope with `truncated`/`total_rows` lands in
provenance; columnar zip produces correct row objects in column order;
duplicate column error; length-mismatch error (check exact text); empty rows
error; data-twice error; declared_field_type table (INT64→quantitative,
DATE→ordinal, STRING→nominal, `whatever`→nominal, case-insensitivity).

**Step 3: Run** `cargo test -p benday-core ingest` — expected: all pass.

**Step 4: `make validate`, commit**

```bash
git add crates/benday-core/src/ingest.rs crates/benday-core/src/lib.rs
git commit -m "feat(core): ingest module — DataDoc, Table, resolve, declared types (unwired)"
```

---

## Task 2: `compile` takes a `&Table`

**Files:**
- Modify: `crates/benday-core/src/compile.rs` (signature + every `spec.data.values` read)
- Modify: `crates/benday-core/src/render.rs` (render signature + tests)
- Modify: `crates/benday-core/tests/corpus.rs`, `crates/benday-core/tests/gallery.rs` (harness call sites)
- Modify: `crates/benday-cli/src/main.rs` (call sites)

**Step 1: Change signatures**

- `compile(spec: &Spec, table: &Table, opts: &CompileOptions) -> Result<Scene, Error>`
- `preflight(spec: &Spec, rows: &[Row])` — drops its empty-rows check (moved to
  `resolve` in Task 1); keeps validate + the three `check_field` calls against
  `rows`.
- `compile_bar`/`compile_xy` read `table.rows` where they read
  `spec.data.values` today. NOTHING else in them changes in this task.
- Public `render` becomes `render(spec: &Spec, data: Option<DataDoc>, opts: &RenderOptions)`;
  it calls `ingest::resolve(spec, data)?` then compile+rasterize. Update the
  six unit tests mechanically (`render(&s, None, &opts())`).

**Step 2: Update harnesses and CLI**

- corpus.rs: `ingest::resolve(&spec, None).and_then(|t| compile(&spec, &t, &opts))`
  — keep snapshotting `Ok`→scene JSON / `Err`→`ERROR (kind): msg` exactly as now.
- gallery.rs: `render(&spec, None, &o)`.
- main.rs: `render(&spec, None, &opts)`; the `--dump-scene` branch resolves then
  compiles. (Real stdin routing is Task 5 — pass `None` for now.)

**Step 3: The referee**

Run: `cargo test --workspace`
Expected: exactly ONE corpus diff — `case__err_empty_data` now reads
`ERROR (data): data has no rows; provide at least one row`. That diff is
authorized: review it, `cargo insta accept`. ZERO gallery diffs. Anything else
diffing means a behavior change slipped in — fix the code.

**Step 4: `make validate`, commit**

```bash
git add -A
git commit -m "refactor(core): compile takes a resolved Table; render accepts an optional data document"
```

---

## Task 3: Spec grammar + declared-type resolution

**Files:**
- Modify: `crates/benday-core/src/spec.rs`
- Modify: `crates/benday-core/src/ingest.rs` (resolve consumes the new forms)
- Modify: `crates/benday-core/src/compile.rs` (type-resolution line, ordinal sort)
- Create: ~8 corpus cases in `crates/benday-core/tests/cases/`

**Step 1: Spec grammar** — `data` optional, columnar form added. In `spec.rs`:

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub data: Option<Data>,
    // ... rest unchanged
}

/// Inline data: tidy row objects, OR columnar `columns` + `rows`. Exactly one
/// form — `ingest::resolve` enforces it (serde can't express either/or here
/// without wrecking error paths).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    #[serde(default)]
    pub values: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(default)]
    pub columns: Option<Vec<Column>>,
    #[serde(default)]
    pub rows: Option<Vec<Vec<serde_json::Value>>>,
}

/// Strict twin of `ingest::EnvColumn`: the spec is agent-authored, so unknown
/// keys are rejected here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
}
```

`resolve` gains the inline-form validation:
`Some(values), None, None` → values form; `None, Some(c), Some(r)` → columnar
(same zip helper as the envelope); any other combination →
`Error::Spec("data must contain either `values`, or `columns` and `rows`")`.
Rules 1–2 from Task 1 now use `spec.data.is_some()` for real.

**Step 2: Declared-type resolution in the compiler.** In `compile_xy`, the two
inference lines become precedence chains:

```rust
let xt = spec.encoding.x.ty
    .or_else(|| table.declared.get(xf).copied())
    .unwrap_or_else(|| data::infer_type(&table.rows, xf));
```

(same for `yt`). `compile_bar`'s y handling is untouched — deliberately, per
the design's scope note: the precedence chain applies where INFERENCE happens
today (xy channels), while bar y keeps its row-by-row `num()` coercion
contract (non-coercible values → `dropped_rows`). A declared `STRING` bar y
whose values parse as numbers still charts; do NOT add a bar-y type gate.
Bars treat x as categorical regardless of type, and `validate`'s
quantitative-x rejection keys off the EXPLICIT spec type only — a declared
INT64 x on a bar chart is fine, it just becomes categories.

**Step 3: Ordinal categories sort.** When the resolved x type is `Ordinal`
(from spec or declared DATE/TIMESTAMP), sort the category list lexically —
ISO date strings then plot chronologically even when rows arrive shuffled.
`Nominal` keeps first-seen order exactly as today.

IMPLEMENTATION TRAP — the index remap. `compile_xy` assigns `xn = category
index` DURING the row scan (`compile.rs:370`) and stores those indices in
series points. Sorting `x_cats` afterward without touching the points sorts
the axis labels while every point still refers to first-seen order — a
silently wrong chart, the exact failure benday exists to prevent. Do it as a
remap after the scan:

```rust
if xt == FieldType::Ordinal {
    let mut sorted = x_cats.clone();
    sorted.sort_unstable();
    // old index -> new index
    let remap: Vec<usize> = x_cats
        .iter()
        .map(|c| sorted.iter().position(|s| s == c).expect("same elements"))
        .collect();
    for s in &mut series {
        for p in &mut s.points {
            p.0 = remap[p.0 as usize] as f64;
        }
    }
    x_cats = sorted;
}
```

(Place it right before the per-series `points.sort_by` so points end up in
sorted-x order too.) `compile_bar` is index-free — categories and groups are
parallel vectors — so there sort the `(cat, group)` pairs together (zip, sort
by cat, unzip) before aggregation. The `declared_date_ordinal` corpus case
with shuffled rows is the test that catches the trap: verify the scene's
points are monotonically increasing in x AND the categories are sorted.
`compile_bar` needs the resolved x type for the sort decision only — compute
it with the same precedence chain; nothing else about bar compilation changes.

**Step 4: New corpus cases** (drop in `tests/cases/`, run, review each scene
against hand-computed expectations, accept):

- `columnar_inline.json` — `"data":{"columns":[{"name":"day","type":"STRING"},{"name":"n","type":"INT64"}],"rows":[["mon",32],["tue",78],["wed",51]]}`, bar mark.
- `declared_beats_inference.json` — line mark; y column typed `STRING` but
  every value numeric-looking (`"12"`, `"7"`): declared wins → must ERROR
  (categorical y), where inference alone would have coerced.
- `spec_beats_declared.json` — same data, but `encoding.y.type: "quantitative"`:
  spec wins → renders.
- `declared_date_ordinal.json` — line mark, x column typed `DATE`, ISO dates
  deliberately SHUFFLED in rows → scene categories must come out sorted.
- `unknown_type_nominal.json` — column typed `GEOGRAPHY` → nominal, renders.
- `err_length_mismatch.json` — 3 columns, one row with 2 values.
- `err_data_both_forms.json` — `values` AND `columns`+`rows` in one data object.
- `err_no_data.json` — spec with no `data` key (resolve with stdin=None).

**Step 5: The referee** — run `cargo test --workspace`: zero diffs to existing
snapshots (gallery AND corpus), 8 new corpus snapshots reviewed and accepted.

**Step 6: `make validate`, commit**

```bash
git add -A
git commit -m "feat(core): optional/columnar spec data; declared column types drive resolution; ordinal categories sort"
```

---

## Task 4: Provenance in the Scene, conditional `--meta` data block

**Files:**
- Modify: `crates/benday-core/src/scene.rs` (`Source` + `meta()`)
- Modify: `crates/benday-core/src/compile.rs` (populate from `table.provenance`)

**Step 1:** `Source` gains three fields (append AFTER existing fields —
declaration order is snapshot order, appending keeps diffs mechanical):

```rust
pub data_source: crate::ingest::DataSource,
pub truncated: Option<bool>,
pub total_rows: Option<u64>,
```

**Step 2:** `Scene::meta()` appends a `data` block ONLY when it carries
information the caller doesn't already have — meta reports what the caller
can't know from their own bytes, and inline data is the caller's own bytes:

```rust
// data block iff: source is stdin, or the envelope reported truncation info
let informative = matches!(self.source.data_source, DataSource::StdinValues | DataSource::StdinColumns)
    || self.source.truncated.is_some()
    || self.source.total_rows.is_some();
```

When present: `{"source": "...", "rows": <row count via series_points sum... NO — use a dedicated field if needed>, "truncated": ..., "total_rows": ...}`.
Simplest correct: `{"source", "truncated", "total_rows"}` (skip row count —
`series_points` already tells the story). Keep it minimal; it's additive later.

**Step 3: The referee** — this task authorizes exactly one class of corpus
diff: every case's `source` object gains `"data_source": "inline_values"` (or
`"inline_columns"` for Task 3's columnar cases), `"truncated": null`,
`"total_rows": null`. Verify NOTHING else changed in any snapshot, then accept.
Gallery must have ZERO diffs — all gallery specs are inline, so the
conditional meta block never fires there. If any gallery bundle diffs, the
conditional is wrong.

**Step 4: `make validate`, commit**

```bash
git add -A
git commit -m "feat(core): data provenance in Scene source; --meta data block for piped/truncated data"
```

---

## Task 5: CLI stdin routing + integration tests

**Files:**
- Modify: `crates/benday-cli/src/main.rs`
- Modify: `crates/benday-cli/Cargo.toml` (dev-deps)
- Create: `crates/benday-cli/tests/cli.rs`

**Step 1: Routing.** Restructure `main`'s input handling:

- Spec via `--spec`/`--spec-file` → parse spec from the flag (existing
  serde_path_to_error block). Then stdin: if `stdin().is_terminal()` → `None`;
  else read to string; if trimmed-empty → `None`; else
  `ingest::parse_data_doc(...)` → `Some(doc)` (parse failure → `fail` with the
  error's kind, exit 3).
- No spec flag → stdin is the spec (existing path, unchanged) — but on spec
  parse FAILURE, add the redirect check before failing generically: parse the
  source as `serde_json::Value`; if it's an array, or an object with a
  `columns` or `rows` key and no `mark` key, fail with
  `"stdin looks like a data document, not a spec; pass the spec via --spec '...' and keep the data on stdin"`
  (kind `spec`, exit 2). Otherwise the existing path-precise error.
- Pass the doc through: `render(&spec, doc, &opts)`; the `--dump-scene` branch
  does `ingest::resolve(&spec, doc)` then `compile`.

**Step 2: dev-deps**

```toml
[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

**Step 3: Integration tests** — `crates/benday-cli/tests/cli.rs`, one test per
row; use `Command::cargo_bin("benday")`, `.write_stdin(...)`, assert on exit
code and stderr/stdout substrings:

| test | setup | expect |
|---|---|---|
| `pipe_envelope_renders` | envelope (with extra `query` obj + `"truncated":true`) on stdin, `--spec` without data, `--meta --no-color` | exit 0, stdout non-empty, stderr contains `"truncated":true` |
| `pipe_bare_array_renders` | `[{"m":"a","v":3},...]` on stdin + `--spec` | exit 0 |
| `spec_on_stdin_still_works` | full spec w/ inline values on stdin, no flags | exit 0 (compat) |
| `data_twice_fails` | envelope on stdin + `--spec` WITH inline values | exit 2, stderr contains `data provided twice` |
| `forgotten_spec_flag_redirects` | envelope on stdin, no flags | exit 2, stderr contains `looks like a data document` |
| `no_data_anywhere_fails` | `--spec` without data, empty stdin | exit 2, stderr contains `no data` |
| `length_mismatch_exit_3` | envelope with a short row + `--spec` | exit 3, stderr contains `2 values but 3 columns` |
| `dump_scene_shows_stdin_source` | envelope + `--spec --dump-scene` | exit 0, stdout contains `"stdin_columns"` |

**Step 4:** Run `cargo test -p benday`. Expected: 8 pass. Then `make validate`.

**Step 5: Commit**

```bash
git add crates/benday-cli
git commit -m "feat(cli): stdin carries data when the spec comes via flag; assert_cmd integration tests"
```

---

## Task 6: Docs + examples

**Files:**
- Modify: `README.md`
- Create: `examples/pipe-demo.sh` (or extend the examples section — executor's judgment)
- Modify: `crates/benday-cli/src/main.rs` (EXAMPLES help text)

**Step 1:** README: the pipe flow gets top billing in usage (it's the point of
the tool); document the two stdin forms, the envelope tolerance, declared-type
mapping (with the DATE→ordinal interim note), precedence rule, and the new
`--meta` data block. Update the spec reference for optional/columnar `data`.
Keep total README growth modest — link the design doc for the reasoning.

**Step 2:** EXAMPLES const in main.rs: add one pipe example
(`query ... | benday --spec '{...}'` shape) and the columnar data form to the
spec sketch.

**Step 3:** `make validate`, final commit, push:

```bash
git add -A
git commit -m "docs: stdin data flow, columnar form, declared types"
git push
```

Confirm CI green. Cycle complete — temporal scales are next, with real piped
data to design against.
