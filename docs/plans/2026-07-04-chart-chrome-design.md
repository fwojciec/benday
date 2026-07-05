# Chart chrome — design

**Date:** 2026-07-04
**Status:** approved
**Scope:** legend relocation + lossless wrapping, palette cap, row-aligned
y-ticks, title breathing room, new default size. Grouped bars and temporal
scales are explicitly OUT.

## Why

Two independent sources converged on the same conclusion this cycle:

- **Dogfood feedback** (another agent using benday on a real workload): the
  legend silently dropped the 5th series — the unlabeled line was still drawn.
  That is the "silently wrong chart" failure mode the README promises against.
  Reproduced on current code: the compiler places legend entries left-to-right
  with no width awareness and the buffer clips at the right edge, so an
  overflowing entry renders as a dangling `─` fragment or vanishes. A related
  silent failure sits in the color channel: `theme.series()` cycles the
  palette, so series 7 reuses series 1's color — indistinguishable lines at
  any width.
- **Visual review** of rendered output: y-tick labels land on irregular row
  gaps (1,2,2,1,2,1 on the default chart) because 7 nice values get rounded
  onto 10 rows — 1.5 rows per tick, with floating-point jitter deciding which
  way each half-row rounds (`(1 − 5/6) × 9 = 1.4999…`). Titles sit directly
  on the top plot row with no separation.

All of it is chrome: the marks are right, the frame around them quietly lies
or stutters. One cycle fixes the frame.

## Vertical layout

Every chart becomes, top to bottom:

```
title row            (only when spec.title present)
blank row            (only when title present — breathing room)
plot rows            (default height 13)
axis row             (└────┴────)
x-label row
legend rows          (only when multi-series; flows + wraps, never clipped)
```

Two moves: the title gains a blank separator row, and the legend relocates
from above the plot to below the x-labels. Terminals extend vertically;
the legend's length now scales with series count, not chart width.

The legend flows entries left-to-right in today's rhythm (`── name` +
3-space gap) starting at the plot's left edge, and **wraps to a new row
whenever the next entry would cross the right edge**. `legend_rows` becomes
a computed count instead of 0-or-1. One honest edge case: a single series
name longer than the whole chart width is visibly truncated with `…` —
visible truncation, never a silent drop.

Compile-side only: the rasterizer already draws legend entries at whatever
`col`/`row` the Scene says.

**Default size: 60×10 → 72×13.** `--width`/`--height` and spec overrides
untouched. Height 13 is deliberate: 12 row-intervals divide by 2, 3, 4, and
6, which feeds the tick alignment below. Width 72 plus gutter stays under 80
columns. Measured cost on a representative bar chart: ~2.2 KB plain /
~3.2 KB colored, vs 1.4 / 2.2 today — affordable for agent transcripts.

Net height cost for a titled multi-series chart: +2 rows, plus wrap rows
only when series overflow one row.

## Row-aligned y-ticks

Today ticks are chosen purely by value niceness (Heckbert 1/2/5 steps), then
each label rounds to its nearest row. New rule: **a tick set is only
acceptable if it lands on uniformly spaced integer rows, at least 2 rows
apart.**

Since the domain is expanded to step multiples, the tick set spans
`k = (max − min) / step` intervals, and rows are even exactly when `k`
divides `plot_h − 1`. Algorithm: start from today's Heckbert step and walk
up the nice ladder (1 → 2 → 5 → 10 → …) until `k` divides `plot_h − 1` with
spacing ≥ 2 rows; take the **smallest acceptable step** (coarser steps can
inflate the niced domain — max 6 becomes max 10 at step 5 — so inflation
only happens when nothing tighter is even). Termination is guaranteed:
`k = 1` (just min and max labeled) divides everything.

The dice chart at the new default: domain 0–6, step 2, four labels
(0/2/4/6) every 4 rows — perfectly even.

Consequences: sometimes fewer labels, in exchange for regular rhythm (which
reads more trustworthy than dense-but-ragged); the row-collision
"first-wins" workaround becomes dead code and is deleted; floating-point
rounding jitter is eliminated as a class (rows are computed by integer
spacing, not per-tick float rounding).

## Palette cap

After series grouping, `series_count > theme.palette.len()` is a hard
error — kind `data`, exit 3:

```
{n} series exceed the {len} distinguishable series colors; aggregate or filter "{color_field}"
```

Reject loudly; the message names the fix, agents self-correct in one retry.
`theme.series()` loses its `%` cycle and becomes a direct index — compile
guards the bound, and the invariant is documented at the call site.

## Testing & snapshot migration

This cycle inverts the referee rule: the default-size change alone diffs
**every** gallery snapshot, so "zero diffs" is meaningless here. Protection
comes from three layers:

1. **The corpus pins semantics.** Scene-level snapshots capture what each
   change claims: tick values *and* rows (even spacing is assertable
   numerically), legend entry positions across a wrap, the title blank row,
   the palette error text. New cases: `tick_rhythm` (the 0–6 domain that
   stutters today), `legend_wrap` (5+ series), `err_palette_exceeded`, plus
   updates where layout shifted.
2. **Unit tests own the algorithm.** The row-aligned step search gets
   table-driven tests in `scale.rs`: (domain, plot_h) → (step, tick count,
   row spacing), including the `k = 1` fallback and
   smallest-acceptable-step selection.
3. **The gallery is re-authored once, reviewed once.** Tasks are ordered so
   glyph output churns early and settles: ticks → layout/legend/title →
   palette error → defaults last. Intermediate re-baselines get a sanity
   diff-read; the full per-snapshot aesthetic review (rendered in a real
   terminal) happens once, on the final gallery. New gallery snapshot: a
   five-series line chart with a wrapped legend.

README example outputs re-render; the `--help` text's default-size lines
update.

## Out of scope (deliberately)

- **Grouped bars** — own cycle; changes spec semantics (a series dimension
  for bar marks), not chrome.
- **Temporal scales** — queued next; benefits from landing after this
  cycle since time ticks build on the same alignment machinery.
- **X-axis tick alignment** — quantitative x columns have the same rounding
  stutter in principle, but it is far less visible (no labels stacked
  beside gaps) and the temporal cycle rewrites x ticks anyway; building
  machinery it would replace is waste.
- **No-color multi-series disambiguation** — with `--no-color`, multiple
  line series are indistinguishable on the canvas regardless of legend.
  Known limitation, recorded here, not addressed.
