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
    "y": { "aggregate": "count" } } }
```

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
by construction). Bin `i` of `n` is a normalized rect `x0 = i/n,
w = 1/n` — bars touch; contiguity is compile-side arithmetic on the
existing `Bar` rect and the rasterizer is untouched. Empty bins follow
the pinned aggregate rules: `count` → zero bar, other aggregates →
`None` → a gap at a stable position.

Rows with a non-numeric value in the binned field are dropped and
counted in `dropped_rows` (existing convention), not errors —
distribution exploration must not die on one dirty row.

## Axis, ticks, meta

The binned x axis IS a linear value axis over `[lo, hi]` with
`step = bin_step` — no new tick machinery. Every edge gets a tick mark;
when the labels cannot all fit, the greedy `place_x_labels` collision
rule thins LABELS while every tick mark stays.

One rule pinned because two rounding conventions exist: edge tick
columns derive from the SAME rounding as bar rect fills
(`round(k/n * plot_w)`, last edge clamped) — NOT `value_axis_x`'s
`(plot_w - 1)` convention — so ticks sit on bar boundaries at every
width. The gallery pins this alignment at several widths.

No new `SceneMark` — binned bars are ordinary
`Bars { direction: Vertical }` whose rects touch, and `XAxis` already
carries `domain`. `Source` gains a skip-serialized
`bin: Option<BinInfo>` (`step`, `domain`, `bins`), keeping every
existing snapshot byte-identical. `--meta` reports:

```jsonc
"x": { "field": "latency_ms", "type": "quantitative",
       "bin": { "step": 10, "domain": [0, 200], "bins": 20 } }
```

— enough for an agent to verify the chart binned as intended.

## Errors that teach (error strings are API)

Seven, one constructor each in `compile/mod.rs`: bin+step+maxbins
together; bin on temporal (points at `timeUnit`); bin on nominal (names
the deciding rung); raw y without aggregate; bin on y; bin on
color / with color splitting; bin with non-bar marks; step producing
more bins than cells (names both fixes).

## Testing

- Bin-selection unit tests sweep `(min, max, plot_w)` combinations:
  edges are nice, coverage is dense, `maxbins` respected, `step`
  verbatim.
- Corpus: auto/maxbins/step cases, an empty-bin zero run under `count`,
  a gap under `mean`, the dirty-row drop, every teaching error.
- Gallery (explicit sizes, never `examples/*.json`): a bell-ish
  histogram, a skewed one with an empty-bin run, a `step: 10` latency
  chart, and tick-alignment pins at several widths.
