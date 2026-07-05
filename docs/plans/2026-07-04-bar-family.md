# Bar family — implementation plan

> **For Claude:** Orchestrator + subagent execution. One subagent per task,
> adversarial review after each. The orchestrator personally runs
> `make validate`, audits every snapshot diff against the task's authorized
> list, and renders charts in a real terminal for tasks 2–5.

**Goal:** Horizontal bars (rankings) and grouped bars (second-dimension
comparison), composing freely, with zero new CLI surface.

**Design:** `docs/plans/2026-07-04-bar-family-design.md` — read it first.

**Architecture:** Compile-side except task 1's rect generalization in the
rasterizer. Orientation is resolved from field types; grouping from the
color field; layout math lives in `compile_bar`, which grows into a small
family of helpers. The chrome cycle's legend/palette machinery is reused
untouched.

**Snapshot referee (STRICT this cycle):** zero diffs in existing gallery
snapshots, all six tasks. Existing-corpus diffs are authorized ONLY in task
1 and ONLY as the mechanical rect change. Everything else lands as NEW
snapshots, each reviewed before acceptance.

---

## Task 1: `Bar` rect generalization + rasterizer rect fill

**Files:**
- Modify: `crates/benday-core/src/scene.rs:124-132` (Bar struct)
- Modify: `crates/benday-core/src/raster/mod.rs:91-144` (rasterize_bars)
- Modify: `crates/benday-core/src/compile.rs` (bar construction, ~line 263)

**Step 1: Scene.** `Bar` becomes a normalized rect:

```rust
/// One bar as a normalized rect over the plot area: `x0/w` as fractions of
/// plot width, `y0/h` as fractions of plot height, y0 = 0 at the TOP (same
/// orientation as point geometry). Vertical bars: y0 = 1 - h, full h to the
/// baseline. Horizontal bars: x0 = 0, w = value fraction.
pub struct Bar {
    pub x0: f64,
    pub y0: f64,
    pub w: f64,
    pub h: f64,
    pub color: Rgb,
}
```

In `compile_bar`, construct with `y0: 1.0 - y.norm(*v), h: y.norm(*v)`
(and today's `x0`/`w` unchanged). Update the `scene.rs` unit test literals.

The `Bars` mark gains an explicit direction — rect anchors CANNOT encode
it (a bottom-row horizontal bar has `x0 == 0` AND `y0 + h == 1`, exactly a
vertical bar's signature):

```rust
#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BarDirection {
    Vertical,
    Horizontal,
}
```

`SceneMark::Bars { bars: Vec<Bar>, direction: BarDirection }` — one field
per mark, not per bar. `compile_bar` sets `Vertical`.

**Step 2: Rasterizer.** `rasterize_bars` takes the direction and fills
rects:

- **Dots:** direction-free — pixel ranges become two-dimensional:
  `px in (x0*plot_w*2).round() .. ((x0+w)*plot_w*2).round()` and
  `py in (y0*plot_h*4).round() .. ((y0+h)*plot_h*4).round()`. For today's
  vertical bars this reproduces `(ph - level)..ph` exactly (y0+h = 1).
- **Blocks:** branch on `direction`. `Vertical` keeps today's bottom-up
  eighths (`EIGHTHS`) fill verbatim. `Horizontal` fills left-anchored with
  a new `LEFT_EIGHTHS` table (`▏▎▍▌▋▊▉`, U+258F down to U+2589, plus `█`)
  for the partial END column.

**Step 3: Rasterizer unit tests** (table-driven, in `raster/mod.rs` tests):
a vertical rect and a horizontal rect, dots and blocks each — assert the
exact glyph rows.

**Step 4:** `make validate`, audit. Authorized: **every corpus snapshot
containing bars** diffs mechanically — each `Bar` gains `"y0": <1 - h>`,
each `Bars` mark gains `"direction": "vertical"`, and nothing else changes
(verify by script: one added line per bar plus one per mark). **Zero
gallery diffs** — this is the referee proving byte-identical vertical
rendering. Zero meta changes.

**Step 5:** Commit: `refactor(scene): bars are normalized rects; rasterizer fills either orientation`

---

## Task 2: Grouped vertical bars

**Files:**
- Modify: `crates/benday-core/src/spec.rs` (add `x_offset` field), 
  `crates/benday-core/src/compile.rs` (validate at :94-100, compile_bar)
- Create: `tests/cases/grouped_bars.json`, `grouped_missing_cell.json`,
  `grouped_agg_mean.json`, `err_xoffset.json`, `err_grouped_palette.json`

**Step 1: Grammar.** In `spec.rs`, add to `Encoding` (serde rename
`xOffset`) a field `x_offset: Option<serde_json::Value>` — `Value`, NOT
`Channel`: Vega-Lite emits several xOffset shapes (`{"field": ...}`,
`{"value": ...}`, band configs) and a strict `Channel` would bounce them
into serde's generic unknown-field error before validation could help. It
exists ONLY to be rejected helpfully in validate:

```rust
if spec.encoding.x_offset.is_some() {
    return Err(Error::Spec(
        "`xOffset` is not supported; grouping is expressed with color alone \
         — set encoding.color to the grouping field".into(),
    ));
}
```

**Step 2: Validate widening.** Delete the bar color==x check
(`compile.rs:94-100`) entirely. The tint-vs-grouped routing moves to the
compile paths and tests against the RESOLVED CATEGORY FIELD, which is x
for vertical and y for horizontal — `color.field == category_field` →
tint, otherwise → grouped. (Task 2 only wires the vertical case; the
predicate is written orientation-neutrally so task 4 reuses it.)

**Step 3: compile_bar grouping.** When `color.field != category field`:

- Scan rows into `cats` (first-seen or ordinal-sorted, as today) ×
  `series_names` (first-seen) with per-(cat, series) value vectors;
  aggregate each cell with the spec aggregate (default sum). Missing cell →
  no bar (the slot stays empty; offsets stay stable because layout indexes
  by series position, not presence).
- **Palette cap** (before layout): `series_names.len() >
  theme.palette.len()` → the chrome cycle's exact error message with the
  color field name. Categorical tint (color==x) stays exempt.
- **Fit check (before layout):** the slots must hold one column per series
  plus an inter-group gap. If `plot_w < n_cats * (n_series + 1)`, error
  (kind data, exit 3):
  `"{n_cats} categories × {n_series} series need width ≥ {req}; raise
  --width, or filter/aggregate"`. Groups NEVER overlap neighbor slots.
- **Layout:** per category slot `step = plot_w as f64 / n_cats as f64`;
  group width `clamp(round(step * 0.7), n_series, floor(step) - 1)`
  columns (the fit check guarantees `floor(step) - 1 >= n_series`), split
  into `n_series` equal bar widths (min 1 col); bars packed adjacent
  within the group, group centered in its slot. Colors `theme.series(si)`.
- **Legend:** build with the chrome cycle's flow+wrap code — extract that
  block from `compile_xy` into a shared
  `fn legend_below(series_names, theme, gutter, columns, top, plot_h) ->
  (Vec<LegendEntry>, usize)` helper and call it from both paths.
  `total_rows` gains the legend rows; bars had none before, so this is
  additive for grouped only.
- **Meta:** in `scene.rs::meta()`, the Bar branch appends a `series` array
  (xy shape: name/color/points from legend entries zipped with
  `source.series_points`) ONLY when the legend is non-empty. Plain and
  tinted bars emit byte-identical meta to today.

**Step 4: Corpus cases.** `grouped_bars` (2 quarters × in/out — the design
doc's referral example), `grouped_missing_cell` (a series absent from one
category — verify the gap and stable offsets in the snapshot),
`grouped_agg_mean`, `err_xoffset` (exact message; use the `{"value": ...}`
Vega-Lite form to prove the Value-typed field catches non-Channel shapes),
`err_grouped_palette` (9 series, the chrome-cycle message verbatim),
`err_group_width` (many categories × series at default width, the fit
error verbatim).

**Step 5:** `make validate`, audit: authorized = the five NEW corpus
snapshots only. Zero existing-snapshot diffs anywhere, gallery included.
Render the referral example in a terminal: groups separated, offsets
stable, legend below, colors match the legend.

**Step 6:** Commit: `feat(compile): grouped bars — color field splits bar series`

---

## Task 3: Horizontal bars (plain)

**Files:**
- Modify: `crates/benday-core/src/compile.rs` (orientation resolution in
  `compile`, new `compile_bar_h`)
- Create: `tests/cases/ranking_horizontal.json`, `horizontal_declared.json`
  (columnar data, declared types drive orientation), `err_both_quant.json`,
  `err_both_categorical.json` (both channels genuinely non-numeric),
  `declared_string_y_still_vertical.json` (columnar, y declared STRING with
  numeric-string values — pins the stdin-cycle coercion contract through
  the rescue), `err_agg_on_category.json` (aggregate on the categorical
  channel, exact message), `horizontal_count.json` (x-count without an x
  field in rows), `err_height_ceiling.json`, `name_truncation.json`

**Step 1: Orientation resolution** in `compile()` (`compile.rs:127-134`),
for `Mark::Bar` only — resolve both channel types through the existing
precedence chain (spec > declared > inference), then:

| x type          | y type          | route                              |
|-----------------|-----------------|------------------------------------|
| nominal/ordinal | quantitative    | `compile_bar` (today)              |
| quantitative    | nominal/ordinal | `compile_bar_h` (new)              |
| quantitative    | quantitative    | error (see below)                  |
| nominal/ordinal | nominal/ordinal | coercion rescue, then error        |

**Coercion rescue (stdin-cycle contract):** the stdin design promises that
a declared-`STRING` y column whose values coerce numerically still charts
as a vertical bar (bar y is not type-gated). So both-categorical first
tries `infer_type(rows, yf) == Quantitative` → vertical (compat bias),
then `infer_type(rows, xf) == Quantitative` → horizontal, and only then
errors:

```text
"bar needs one categorical and one quantitative channel; both x (\"{xf}\")
and y (\"{yf}\") resolved {both}; put categories on one axis or set an
explicit \"type\"" — kind spec, exit 2
```

(Two variants, `quantitative` / `categorical`; both-quantitative gets no
rescue — an explicit `"type"` is the fix.) Every existing bar corpus case
has categorical x and numeric y and must keep its route: zero
existing-corpus diffs.

**Step 1b: Aggregate placement and field checks move post-orientation.**
`validate()`'s blanket "aggregate on encoding.x is not supported"
(`compile.rs:81-86`) now applies to NON-bar marks only (message unchanged).
Bars check at the top of each compiler, after orientation is known:
aggregate on the categorical channel → error (kind spec)
`"aggregation runs over the quantitative channel, grouped by the
categorical one; put `aggregate` on encoding.{value_axis}"`. Likewise
`preflight`'s field checks (`compile.rs:68-78`) become orientation-aware
for bars: the CATEGORY field must exist in rows; the VALUE field must
exist unless its aggregate is `count`. `compile_bar_h` reads
`agg = spec.encoding.x.aggregate.unwrap_or(Aggregate::Sum)` — count
yields 1.0 per row without reading the x field, mirroring vertical
y-count.

**Step 2: `compile_bar_h`.** Mirrors `compile_bar`'s scan with axes
swapped: categories from the Y field (first-seen; ordinal sorts), values
from `num(x)`, negatives rejected with the existing message.

- **Content-sized height:** `plot_h = n_bars + gaps` where plain bars get
  one row each and one blank row between categories (`n_cats * 2 - 1`
  plain). CRITICAL PLUMBING: `plot_dims()` collapses "no height" into the
  default 13 before dispatch, so `compile_bar_h` cannot tell a user's 13
  from the default — it must NOT go through `plot_dims` for height. Pass
  the RAW `opts.height.or(spec.height): Option<usize>` into
  `compile_bar_h` (width still comes from `plot_dims`; the vertical path
  is untouched). `Some(ceiling)`: `content > ceiling` → error
  `"{n} bars need height {h}; filter or aggregate, or raise --height"`
  (kind data, exit 3). `None`: safety cap 40 rows, same message with
  "raise --height" as the escape.
- **Name gutter:** `gutter = max(truncate(name, 24).width)` over
  categories; names right-aligned, one per bar-block row (`YTick` entries:
  value = category index as f64, frac over [0, n-1], label = truncated
  name, row = the bar's row). The y rule `│` and `┤` glyphs render from
  existing raster code untouched.
- **Value axis:** `Linear::nice_from(0, vmax, (plot_w / 10).clamp(2, 7),
  true)` on x — reuse the quantitative x-tick block from `compile_xy`
  (`compile.rs:551-563`) verbatim: tick cols, `fmt_tick` labels, greedy
  placement. Extract it into a shared helper `fn value_axis_x(...)` used
  by both.
- **Bars:** `Bar { x0: 0.0, y0: row / rows, w: xscale.norm(v),
  h: 1.0 / rows, color }`, direction `Horizontal` — gradient by
  `xscale.norm(v)` (plain) or `theme.series(i)` (tint: `color.field`
  equals the CATEGORY field, which is y here — the orientation-neutral
  predicate from task 2).
- **Meta:** the Bar branch reports
  `x: {field, type: "quantitative", domain}` and
  `y: {field, type: "nominal", categories}` when horizontal (detect:
  `x_axis.categories.is_none()` for a bar scene) plus
  `"direction": "horizontal"`. Vertical meta byte-identical to today.

**Step 3: Corpus cases** as listed in Files. `ranking_horizontal` uses
realistic long facility names in ranked (SQL) order; verify in the
snapshot: first-seen order preserved, `size.rows` content-derived, names
truncated at 24 with `…` (in `name_truncation`), value ticks on x.

**Step 4:** `make validate`, audit: new snapshots only; zero existing
diffs. Render the ranking in a terminal: names readable, bars aligned,
value axis labeled.

**Step 5:** Commit: `feat(compile): horizontal bars — quantitative x + categorical y`

---

## Task 4: Grouped horizontal

**Files:**
- Modify: `crates/benday-core/src/compile.rs` (compile_bar_h grows the
  series dimension)
- Create: `tests/cases/grouped_horizontal.json`,
  `grouped_horizontal_missing.json`, `err_h_height_ceiling_grouped.json`

**Step 1:** `compile_bar_h` accepts the third-field color case (task 2's
scan transposed): per category block, `n_series` bar rows (one per series,
stable order) + one blank row between categories —
`n_cats * (n_series + 1) - 1` rows total, same ceiling rules. Category
name centered on its block (`row = block_start + (n_series - 1) / 2`).
Legend below via the shared helper. Palette cap identical to task 2.
Missing cell → its row stays empty (stable offsets).

**Step 2: Corpus cases:** `grouped_horizontal` (facilities × in/out,
ranked), `grouped_horizontal_missing`, `err_h_height_ceiling_grouped`
(the count in the message must reflect `n_cats × n_series`).

**Step 3:** `make validate`, audit (new snapshots only), terminal render:
blocks separated, names centered per block, legend below.

**Step 4:** Commit: `feat(compile): grouped horizontal bars`

---

## Task 5: Gallery, docs

**Files:**
- Modify: `crates/benday-core/tests/gallery.rs`, `README.md`,
  `crates/benday-cli/src/main.rs` (EXAMPLES text)

**Step 1: Gallery snapshots** as EXPLICIT cases with their own sizes (NOT
`examples/*.json` — the examples loop renders at 30×6, which a
content-sized ranking correctly rejects as over-ceiling):
`ranking_horizontal` (8 facilities, long names, 60 wide),
`grouped_bars_referrals` (vertical, 2 series), `grouped_horizontal_small`,
and one ANSI variant `grouped_bars_ansi` (legend + series colors under
color). Reuse the corpus case JSON inline.

**Step 2: README.** "The spec" section: note the orientation rule
(quantitative x + categorical y = horizontal) and color-as-grouping on
bars; move horizontal/grouped bars from Planned to Works. EXAMPLES in
`--help`: one horizontal line under the spec sketch. Status section:
remove "negative and horizontal bars" (negatives remain out), add
"stacked bars" to Planned if desired — or leave Planned as-is minus
horizontal.

**Step 3:** `make validate` green; new gallery snapshots reviewed
per-snapshot in a real terminal (orchestrator), then accepted; `git
status` clean; push and confirm CI.

**Step 4:** Commit: `docs: bar family — gallery snapshots, README`

---

## Execution notes

- Task 1 is the referee-critical one: its gallery zero-diff proves the
  rect refactor is behavior-identical. Gate on it before task 2.
- Tasks 2 and 3 are independent of each other but both build on 1; run
  them sequentially anyway (shared helpers land in 2: `legend_below`;
  in 3: `value_axis_x`).
- The orchestrator personally reviews the orientation-resolution table
  (task 3 step 1) and the content-sized height arithmetic before task 4
  builds on both.
- Every commit leaves `make validate` green; CI runs exactly
  `make validate`.
