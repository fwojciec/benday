# Histograms (`bin`) — design

**Date:** 2026-07-05
**Status:** approved
**Scope:** a `bin` encoding property for quantitative x on bars —
automatic nice bins, `maxbins`, `step` — rendered as contiguous rects on
a linear edge-ticked axis. Bin on y/color, overlaid histograms, and
non-bar marks are explicitly OUT.

## Why

The distribution question — "what does this column look like?" — is the
analysis beat bars and lines cannot express, and it came second only to
temporal in the 2026-07-05 discovery. Both real workloads want it: log
debugging (latency and payload-size distributions, spotting bimodality
after an incident) and cube exploration (order-size and reimbursement
distributions). The agent's alternative today is binning by hand in SQL
or code — the same friction `timeUnit` just removed for time, and the
same lesson applies: when there is no SQL in the loop, the tool must own
the transform.

## Spec semantics

```jsonc
{ "mark": "bar",
  "encoding": {
    "x": { "field": "latency_ms", "bin": true },   // or {"maxbins": 15} / {"step": 10}
    "y": { "field": "latency_ms", "aggregate": "count" } } }
```

`field` stays REQUIRED on every channel this cycle, count included —
`Channel.field` is a non-optional `String` and the existing convention
(bar family) is that count's field may be absent from the ROWS, not from
the spec. An agent emitting Vega-Lite's fieldless
`{"aggregate": "count"}` gets serde's path-precise missing-field error
naming `encoding.y.field`. Fieldless count is a possible future
ergonomics change, out of this cycle's scope.

- Three `bin` shapes, nothing else: `true` (automatic), `{"maxbins": N}`,
  `{"step": w}`. `maxbins` and `step` together is an error — they fight.
- The binned field must resolve QUANTITATIVE via the standard precedence
  chain. Temporal fields already have `timeUnit`; the error for `bin` on
  temporal points there. `bin` on a nominal field names the resolved
  type and the rung that decided it.
- y must carry an EXPLICIT aggregate. `count` is the histogram;
  `mean`/`sum`/… over binned x ("mean latency by size bucket") comes
  free from the existing aggregate machinery. Raw quantitative y with
  binned x is a teaching error: "binned x groups many rows per bar; add
  an aggregate to y". No default-to-count magic — agents declare intent.
- OUT, each rejected with a teaching error (out means rejected, not
  fall-through): `bin` on y (suggests swapping axes), `bin` on color,
  `bin` with `line`/`point`/`area`, `bin` + `timeUnit` on one channel,
  `bin` + `color` series-splitting (overlaid histograms cannot overlap
  legibly in braille — future cycle if dogfooding demands).

## Bin selection

Automatic bins reuse `nice_num` — the same 1/2/5 ladder the y axis
walks:

- `target = (plot_w / 4).clamp(5, 20)` — 18 bins of ~4 cells at the
  default 72-cell plot; 7 fat bins at a narrow 30.
- Degenerate spans take the SAME guard as `Linear::nice_from`
  (`scale.rs`): if `max - min` is not normal (all values equal, or one
  row), treat the span as `min..min + 1.0` before selection — every
  value lands in one bin and nothing divides by zero. Zero usable
  numeric rows is the existing "no usable rows" error.
- `step = nice_num((max - min) / target)`; domain expands outward to
  step multiples: `lo = floor(min/step)*step`, `hi = ceil(max/step)*step`.
  Every edge is a nice number BY CONSTRUCTION — the axis section
  exploits this.
- `maxbins: N`: same algorithm with `target = N`, then coarsen up the
  ladder while the count exceeds N.
- `step: w`: the caller's width verbatim, edges floored/ceiled to
  multiples of `w`. More bins than plot cells is a teaching error naming
  the count, the width, and both fixes (wider `step` or wider
  `--width`).

## Geometry

Bins are generated DENSE over `[lo, hi]` — every bin exists, occupied or
not, so the silhouette never lies (the timeUnit densify lesson, applied
by construction).

Contiguity must be computed in INTEGER CELLS, not normalized fractions:
the rasterizer rounds each rect's `x0` and `w` independently
(`raster/mod.rs` dots-vertical fill), so naive `x0 = i/n, w = 1/n` bars
overlap and overflow (`plot_w = 10, n = 4` yields spans
`[0,3) [3,6) [5,8) [8,11)`). Compile therefore rounds the EDGES first —
`edge_k = round(k/n * plot_w)` for `k = 0..=n` — and stores
`x0 = edge_i / plot_w`, `w = (edge_{i+1} - edge_i) / plot_w`; the
rasterizer's independent rounding then recovers those exact integers,
bars tile the plot with no gap, no overlap, no overflow, and the
rasterizer stays untouched. (Consequence: at `plot_w < n` some bins are
zero cells wide — the `step` error below prevents the caller-forced
case, and automatic selection targets ≥ ~4 cells per bin.)

Empty bins follow the pinned aggregate rules: `count` → zero bar, other
aggregates → `None` → a gap at a stable position.

Dirty rows: dropping a non-numeric value into `dropped_rows` (existing
convention) applies ONLY when the field resolved quantitative by
EXPLICIT spec type or DECLARED column type. An undeclared column with
one dirty value infers Nominal under the existing all-or-nothing
`infer_type` rule — that is the bin-on-nominal teaching error, whose
message therefore also names the escape: set
`encoding.x.type = "quantitative"` if the column is numeric with dirt.
Inference itself does not change.

## Axis, ticks, meta

The binned x axis IS a linear value axis over `[lo, hi]` with
`step = bin_step` — no new tick machinery. Every edge gets a tick mark;
when the labels cannot all fit, the greedy `place_x_labels` collision
rule thins LABELS while every tick mark stays.

One rule pinned because two rounding conventions exist: edge tick
columns are the SAME integers as the rect edges above
(`edge_k = round(k/n * plot_w)`, last edge clamped to the plot) — NOT
`value_axis_x`'s `(plot_w - 1)` convention — so ticks sit on bar
boundaries at every width by construction. The gallery pins this
alignment at several widths.

No new `SceneMark` — binned bars are ordinary
`Bars { direction: Vertical }` whose rects touch, and `XAxis` already
carries `domain`. `Source` gains a skip-serialized
`bin: Option<BinInfo>` (`step`, `domain`, `bins`), keeping every
existing snapshot byte-identical.

**The bar meta detector must change.** `Scene::meta()`'s bar branch
currently reads `x_axis.categories.is_none()` as "horizontal" and then
EXPECTS y categories — a histogram scene (vertical, domain-carrying x)
would panic. The detector re-keys on the `Bars` mark's explicit
`direction` (carried since the bar family): Horizontal → today's shape
byte-identically; Vertical + x categories → today's nominal/timeUnit
shape byte-identically; Vertical + no x categories → the NEW histogram
shape below. Zero meta diffs for every existing chart. `--meta`
reports:

```jsonc
"x": { "field": "latency_ms", "type": "quantitative",
       "bin": { "step": 10, "domain": [0, 200], "bins": 20 } }
```

— enough for an agent to verify the chart binned as intended.

## Errors that teach (error strings are API)

Ten, one constructor each in `compile/mod.rs`:

1. `maxbins` and `step` together — they fight; pick one.
2. `bin` on a temporal field — points at `timeUnit`.
3. `bin` on a nominal field — names the resolved type, the deciding
   precedence rung, and the dirty-numeric escape
   (`"type": "quantitative"`).
4. Binned x with a raw quantitative y (no aggregate) — "binned x groups
   many rows per bar; add an aggregate to y".
5. `bin` on the y channel — suggests swapping axes.
6. `bin` on the color channel.
7. `color` series-splitting alongside a binned x — overlaid histograms
   are out; suggests dropping color or pre-filtering.
8. `bin` with `line`/`point`/`area` — bin is bar-only this cycle.
9. `bin` and `timeUnit` on the same channel — one transform per
   channel.
10. `step` producing more bins than plot cells — names the count, the
    width, and both fixes (wider `step` or wider `--width`).

## Testing

- Bin-selection unit tests sweep `(min, max, plot_w)` combinations:
  edges are nice, coverage is dense, `maxbins` respected, `step`
  verbatim.
- Corpus: auto/maxbins/step cases, an empty-bin zero run under `count`,
  a gap under `mean`, the dirty-row drop, every teaching error.
- Gallery (explicit sizes, never `examples/*.json`): a bell-ish
  histogram, a skewed one with an empty-bin run, a `step: 10` latency
  chart, and tick-alignment pins at several widths.
