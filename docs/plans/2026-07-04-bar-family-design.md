# Bar family — design

**Date:** 2026-07-04
**Status:** approved
**Scope:** horizontal bars (rankings) and grouped bars (second-dimension
comparison), composing freely. Stacking, diverging bars, and sort grammar are
explicitly OUT.

## Why

The real workload (established by dogfooding and by the shape of the data
benday pairs with) is analytical cubes queried by free-range SQL agents:
rankings of opportunities by doctor/facility under various measures, volumes
per doctor per procedure per quarter, referral flows in/out per quarter,
reimbursement data. Two chart shapes dominate and neither is expressible
today:

- **Rankings** — sorted magnitude per entity with long names. Vertical bars
  serve this badly: 12-char x-label truncation mangles entity names and 15+
  entities don't fit as columns. The natural form is horizontal bars: names
  get a gutter column, entities stack vertically, terminals scroll
  vertically.
- **Second-dimension comparison** — in/out referrals per quarter, volumes per
  facility per quarter. That is grouped bars; today `bar` + `color` can only
  tint by the x category itself.

The key design constraint: the callers are agents composing SQL. SQL already
owns sorting (`ORDER BY` → first-seen category order → ranked bars),
bucketing (`date_trunc`), and label formatting (`FORMAT_DATE`). benday owns
only what SQL cannot express: **layout and geometry**. (This is also why
temporal scales left the roadmap: declared `DATE → ordinal` sorting plus SQL
formatting already covers period axes for cube data.)

## Spec semantics

**One rule resolves orientation: bars run from the nominal axis toward the
quantitative one.**

- `x` nominal/ordinal + `y` quantitative → vertical bars (today, unchanged).
- `x` quantitative + `y` nominal/ordinal → **horizontal bars**. No new spec
  surface — this is the Vega-Lite convention agents already emit. Type
  resolution uses the existing precedence chain (spec `type` > declared
  column type > inference), so a piped envelope with `facility STRING,
  volume INT64` orients correctly with no annotations.
- `aggregate: "count"` decides FIRST: it makes its channel the
  quantitative value channel regardless of field type (count is
  intrinsically numeric; its field may be absent from rows, which would
  otherwise infer nominal and misroute). Count on both channels is an
  error.
- Both quantitative → error naming the rule and both resolved types.
- Both categorical → **coercion rescue** before erroring, honoring the
  stdin cycle's contract that a declared-`STRING` y column of
  numeric-string values still charts as a vertical bar (bar y is not
  type-gated). The rescue applies only to channels WITHOUT an explicit
  spec `"type"` — an explicit type is the caller's stated intent and is
  never overridden. Rescue order: y coerces numeric → vertical (compat
  bias), else x coerces → horizontal, else the both-categorical error.

**`color` means what it means everywhere else: the field that splits
series.**

- No `color` → plain bars (gradient fill, unchanged).
- `color` field = the category field → categorical tint (unchanged).
- `color` field = a third field → **grouped bars**: one bar per series
  within each category, colored by series, legend below the chart (the
  chrome cycle's wrap machinery, reused as-is). Today this case is an
  error, so the widening breaks nothing.
- `xOffset` stays rejected, but its error names the fix: "grouping is
  expressed with color alone".

**Aggregate lives on the quantitative (value) channel.** Vertical bars:
`y.aggregate`, exactly as today. Horizontal bars: `x.aggregate` — the value
channel moved, the aggregate moves with it. The opposite placement is an
error naming the rule ("aggregation runs over the quantitative channel,
grouped by the categorical one"). `count` on the value channel works with
the field otherwise absent from rows, mirroring today's y-count behavior.
Because orientation is resolved from data-dependent types, these checks run
after orientation resolution, not in the pure-spec validate pass.

**Sort order stays SQL's job.** Nominal categories keep first-seen order —
`ORDER BY volume DESC` in the query IS the ranking. Ordinal (declared dates,
explicit type) keeps the lexical sort from the stdin cycle. benday adds no
sort grammar.

Grouping and orientation compose orthogonally: `x` quantitative + `y`
nominal + `color` third-field = grouped horizontal.

## Layout

**Vertical grouped:** each category owns a slot of `plot_w / n_cats`
columns; inside it, one bar per series member at ~70% of the slot split
evenly, minimum 1 column each, and at least a 1-column gap preserved
between groups. Groups must FIT: when `plot_w < n_cats × (n_series + 1)`
the slots cannot hold one column per series plus the gap, and that is a
loud error naming the required width — never an overlap into the neighbor
slot. A series member absent from a category leaves its column empty — a
visible gap at a stable position (group members keep the same offset in
every group, so color and position both identify them). Legend below,
exactly as multi-series lines render it.

**Horizontal:** the axes swap jobs. The value axis is the bottom x axis and
reuses today's quantitative x-tick code verbatim — ticks, greedy labels,
`┴` glyphs, nothing new. The left gutter holds **category names** instead
of numbers: width = longest name, capped at 24 cells with `…` truncation
(visible, never silent). Plot height is **content-sized**: one row per bar
plus a blank row between categories — grouped means `n_cats × n_series` bar
rows plus gaps. `--height`/`spec.height` become a ceiling: content
exceeding it is a loud error ("32 bars need height 47; filter or aggregate,
or raise --height"), never a squeeze into sub-row slivers. A safety cap
(~40 rows) applies when no height is given.

**Scene:** `Bar {x0, w, h}` generalizes to a normalized rect
`{x0, y0, w, h}` — vertical bars are `y0 = 1 − h`, horizontal are
`x0 = 0, w = value`. The `Bars` mark carries an explicit `direction`:
dot fill is genuinely orientation-free (fill the rect), but block-style
partial caps are not (bottom-up eighths vs left-anchored eighths), and
rect anchors alone cannot distinguish a bottom-row horizontal bar
(`x0 = 0` AND `y0 + h = 1`) from a vertical one — so the compiler states
the direction rather than the rasterizer guessing. This is the cycle's one
rasterizer touch.

## Palette cap, errors, meta

**The palette cap extends to grouped bars.** The chrome cycle's rule was
"color as the sole identifying channel caps at the palette" — grouped bars
sit near the boundary (within-group offset gives some position identity),
but nine-plus bars per group are sub-column slivers long before color runs
out, so the cap applies wherever `color` names a third field, on any mark.
The categorical-tint exemption (color = category field, bars individually
labeled) stays.

**Errors, all with the fix in the message:**

- Both channels quantitative, or both categorical (after the coercion
  rescue fails) → the orientation rule plus both resolved types.
- `aggregate` on both channels → "aggregate belongs on exactly one
  channel".
- `xOffset` present → accepted by the grammar, rejected in validation with
  "grouping is expressed with color alone" (the only way to beat
  `deny_unknown_fields`' generic message to a helpful one).
- Negative values → still rejected for bars, both orientations.
- Aggregate on the categorical channel → "aggregation runs over the
  quantitative channel, grouped by the categorical one".
- Content height over the ceiling → the count, the required height, and
  the three ways out.
- Grouped width overflow → categories × series that cannot fit one column
  per bar plus inter-group gaps names the required width.
- Series over palette → the chrome cycle's message, verbatim.

Grouped cells aggregate per (category, series) pair with the same default
`sum` — the existing per-category logic, one key deeper.

**Meta stays append-only and conditional**, same doctrine as the stdin
`data` block: grouped bars add a `series` array (name, color, cell count —
the xy shape); horizontal adds `"direction": "horizontal"`. Plain vertical
bars emit byte-identical meta to today — which keeps every existing gallery
bundle untouched.

## Testing & referee

**This cycle returns to the strict referee: zero diffs in existing gallery
snapshots.** Everything is additive — new modes render where nothing
rendered before. The one internal change touching existing charts, the
`Bar` rect generalization, must produce byte-identical glyphs for vertical
bars; the gallery is the instrument that proves it.

- **Corpus:** existing bar cases diff mechanically and uniformly
  (`{x0,w,h}` → `{x0,y0,w,h}`, `y0 = 1 − h`, nothing else) — one enumerated
  authorization, verifiable by script. New cases pin the semantics:
  orientation resolved from declared types (a piped-envelope shape),
  both-quantitative and both-categorical errors, the `xOffset` redirect,
  grouped basic + missing-cell gap + per-cell aggregation, grouped
  horizontal composition, content-sized height landing in `Scene.size`,
  the height-ceiling error, the palette cap on grouped, 24-char name
  truncation with `…`.
- **Rasterizer:** table-driven rect-fill tests, both orientations ×
  dots/blocks — the one place this cycle touches glyph code.
- **New gallery snapshots** (each reviewed before acceptance): a facility
  ranking with long names, grouped vertical (in/out referrals per quarter),
  grouped horizontal, plus ANSI variants for the grouped legend.
- **CLI: no changes at all** — no new flags; orientation and grouping live
  entirely in the spec.

## Out of scope (deliberately)

- **Stacked bars** — `color` groups rather than stacks; a documented
  divergence from Vega-Lite's stacking default.
- **Diverging / negative bars** — negatives stay an error.
- **Sort grammar** — SQL owns order.
- **`xOffset` as a working channel** — one spelling per intent.
- **Value labels at bar ends** — a real candidate for rankings; noted for
  a future polish cycle.
