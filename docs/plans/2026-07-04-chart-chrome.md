# Chart chrome — implementation plan

> **For Claude:** Orchestrator + subagent execution. One subagent per task,
> adversarial review after each. The orchestrator personally runs
> `make validate`, inspects every snapshot diff against the task's authorized
> list, and renders charts in a real terminal for tasks 2, 3, and 6.

**Goal:** Fix the chrome around the marks: row-aligned y-ticks, title
breathing room, a lossless legend below the plot, a hard cap on
indistinguishable series colors, and a 72×13 default.

**Design:** `docs/plans/2026-07-04-chart-chrome-design.md` — read it first.

**Architecture:** All compile-side. The rasterizer draws whatever the Scene
says and is untouched. `scale.rs` gains the row-aligned step search;
`compile.rs` gains a shared y-tick builder and the new vertical layout;
`theme.rs` gets one doc comment. No spec grammar changes, no ingest changes.

**Snapshot referee for this cycle (inverted):** gallery and corpus diffs are
EXPECTED — each task enumerates exactly which snapshots may change and why.
A diff outside the task's list is a bug. Accept snapshots only after the
per-snapshot review described in each task (`cargo insta review`, or
`cargo insta accept` after the orchestrator has read the pending diffs).
Existing CLI `assert_cmd` tests are size-insensitive and must keep passing
untouched through tasks 1–4; task 5 may not touch them either (verify).

---

## Task 1: `Linear::row_aligned` — the step search (not wired)

**Files:**
- Modify: `crates/benday-core/src/scale.rs`

Pure addition; nothing calls it yet. Zero snapshot diffs authorized.

**Step 1: Write the failing tests** (append to `mod tests` in `scale.rs`):

```rust
#[test]
fn row_aligned_keeps_fine_step_when_even() {
    // 12 intervals, k=6 divides: step 1 survives, all 7 labels.
    let s = Linear::row_aligned(0.0, 6.0, 6, 13, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 6.0, 1.0));
}

#[test]
fn row_aligned_coarsens_to_divide_rows() {
    // 9 intervals: k=6 fails (9 % 6 != 0), step 2 gives k=3, spacing 3.
    let s = Linear::row_aligned(0.0, 6.0, 6, 10, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 6.0, 2.0));
}

#[test]
fn row_aligned_climbs_ladder_past_domain_inflation() {
    // h7: 6 intervals. step 50 -> k=4 (no), step 100 -> k=2, spacing 3.
    let s = Linear::row_aligned(0.0, 160.0, 6, 7, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 200.0, 100.0));
}

#[test]
fn row_aligned_min_spacing_forces_fallback() {
    // h6: 5 intervals (prime). step 2 -> k=5, spacing 1 (rejected);
    // step 5 -> k=2 (5 % 2 != 0); step 10 -> k=1: min/max only.
    let s = Linear::row_aligned(0.0, 10.0, 6, 6, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 10.0, 10.0));
    assert_eq!(s.ticks(), vec![0.0, 10.0]);
}

#[test]
fn row_aligned_negative_domain_no_zero() {
    // step 10 -> k=5 (12 % 5 != 0); step 20 -> k=3, spacing 4.
    let s = Linear::row_aligned(-20.0, 30.0, 6, 13, false);
    assert_eq!((s.min, s.max, s.step), (-20.0, 40.0, 20.0));
}

#[test]
fn row_aligned_fractional_step() {
    // step 0.5 -> domain 0..2, k=4, 12 % 4 = 0, spacing 3.
    let s = Linear::row_aligned(0.0, 1.7, 6, 13, true);
    assert_eq!((s.min, s.max, s.step), (0.0, 2.0, 0.5));
}
```

**Step 2:** `cargo test -p benday-core row_aligned` — expect: does not compile
(`row_aligned` not found).

**Step 3: Implement** (in `impl Linear`, after `nice_from`; `next_nice` as a
free function next to `nice_num`):

```rust
/// Like `nice_from`, but for a y axis drawn on `rows` terminal rows: the
/// step is coarsened up the 1/2/5 ladder until the tick intervals divide
/// the row intervals exactly, so every tick lands on a uniformly spaced
/// integer row at least 2 rows from its neighbor. Terminates because a
/// step spanning the whole domain gives k = 1, which divides everything.
pub fn row_aligned(
    mut min: f64,
    mut max: f64,
    target_ticks: usize,
    rows: usize,
    include_zero: bool,
) -> Self {
    if include_zero {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    if !(max - min).is_normal() {
        max = min + 1.0;
    }
    let intervals = rows.max(3) - 1;
    let mut step = nice_num((max - min) / (target_ticks.max(2) - 1) as f64, true);
    loop {
        let lo = (min / step).floor() * step;
        let hi = (max / step).ceil() * step;
        let k = ((hi - lo) / step).round() as usize;
        if k >= 1 && intervals % k == 0 && intervals / k >= 2 {
            return Linear { min: lo, max: hi, step };
        }
        step = next_nice(step);
    }
}
```

```rust
/// The next step up the 1/2/5 ladder (1 -> 2 -> 5 -> 10 -> 20 ...).
fn next_nice(step: f64) -> f64 {
    let exp = step.log10().floor();
    let pow = 10f64.powf(exp);
    let f = (step / pow).round();
    if f < 2.0 {
        2.0 * pow
    } else if f < 5.0 {
        5.0 * pow
    } else {
        10.0 * pow
    }
}
```

**Step 4:** `make validate` — all green, zero pending snapshots
(`cargo insta pending-snapshots` is empty).

**Step 5:** Commit: `feat(scale): row-aligned tick step search`

---

## Task 2: Wire row-aligned ticks into compile

**Files:**
- Modify: `crates/benday-core/src/compile.rs`
- Create: `crates/benday-core/tests/cases/tick_rhythm.json`
- Rename: `tick_collision_h7` gallery case → `small_height_ticks_h7`

**Step 1: Shared tick builder.** Both compile paths currently duplicate the
tick loop (`compile.rs:231-245` bar, `compile.rs:501-515` xy). Replace both
with one helper (place it near `plot_dims`):

```rust
/// Y ticks for a row-aligned scale: k intervals over plot_h-1 rows with
/// exact integer spacing, one YTick per scale tick, rows descending from
/// the bottom. `top` is the plot's buffer-absolute first row.
fn y_ticks(y: &Linear, plot_h: usize, top: usize) -> Vec<YTick> {
    let k = ((y.max - y.min) / y.step).round() as usize;
    let spacing = (plot_h - 1) / k;
    y.ticks()
        .iter()
        .enumerate()
        .map(|(i, &t)| YTick {
            value: t,
            frac: y.norm(t),
            label: fmt_tick(t, y.step),
            row: top + (plot_h - 1) - i * spacing,
        })
        .collect()
}
```

Both call sites become `let ticks = y_ticks(&y, plot_h, top);` (xy:
`&yscale`). Delete the `used: HashSet` first-wins dedup in both, and the
`use std::collections::HashSet;` import if nothing else uses it (check).

**Step 2: Switch the two scale constructors.**
- Bar (`compile.rs:204`): `Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true)`
- XY (`compile.rs:454`): `Linear::row_aligned(ymin, ymax, plot_h.clamp(3, 6), plot_h, mark == Mark::Area)`

The x scale (`compile.rs:456`) stays `nice_from` — x alignment is out of
scope (design doc, "Out of scope").

**Step 3: New corpus case** `tests/cases/tick_rhythm.json` — the domain that
stutters today:

```json
{
  "data": { "values": [
    {"face": "1", "wins": 3}, {"face": "2", "wins": 3},
    {"face": "3", "wins": 3}, {"face": "4", "wins": 4},
    {"face": "5", "wins": 1}, {"face": "6", "wins": 6}
  ]},
  "mark": "bar",
  "encoding": { "x": {"field": "face"}, "y": {"field": "wins"} }
}
```

**Step 4: Rename the misnomer.** In `gallery.rs`, rename the
`tick_collision_h7` case (and its two mentions in the options plumbing at
`gallery.rs:87-105`) to `small_height_ticks_h7`; `git rm` the old snapshot
file `tests/snapshots/gallery__tick_collision_h7.snap` (use `git rm`/`git
add` explicitly — `commit -am` misses renames).

**Step 5:** `make validate`, then review pending snapshots. Authorized diffs:
- **Corpus: any case's `y_axis` block** (tick rows now uniformly spaced;
  `step`/`domain` coarsened where the old k didn't divide). Where the domain
  inflated, `marks` geometry shifts too (renormalized against the wider
  domain) — verify by hand on at least `negative_line`-style cases that the
  new domain is what the algorithm predicts.
- **Gallery: every snapshot may diff in tick label rows**; marks diff only
  where the corpus shows a domain change for the same shape. In the
  30×6 snaps expect min/max-only labels (5 prime intervals — design doc).
- **Verify the rhythm claim directly:** in every diffed snapshot, tick rows
  must be evenly spaced. The orchestrator renders the dice chart
  (`tick_rhythm` data, height 10 and 13) in a terminal and checks even gaps.

Zero diffs allowed in: legend positions, title rows, x labels, error texts.

**Step 6:** Commit: `feat(compile): row-aligned y ticks — uniform integer row spacing`

---

## Task 3: Vertical layout — title breathing row, legend below plot

**Files:**
- Modify: `crates/benday-core/src/compile.rs`
- Create: `crates/benday-core/tests/cases/legend_wrap.json`

**Step 1: compile_bar layout** (`compile.rs:209`): change
`let title_rows = usize::from(spec.title.is_some());` to

```rust
// Title gets a blank row beneath it — breathing room (design doc).
let title_rows = if spec.title.is_some() { 2 } else { 0 };
```

Delete the `legend_rows` local (bars have no legend; `total_rows =
title_rows + plot_h + 2` stays as-is once `legend_rows` is gone, and
`top = title_rows`).

**Step 2: compile_xy layout** (`compile.rs:463-499`): replace the
title/legend layout block. Title handling matches bar (`title_rows` 2-or-0,
`top = title_rows`). The legend moves BELOW the x labels and wraps:

```rust
// Legend (multi-series only): "── name" entries flow below the x labels,
// wrapping before the right edge. Entries are never clipped; a name wider
// than the whole row is visibly truncated with '…'.
let legend_row0 = top + plot_h + 2;
let mut legend: Vec<LegendEntry> = Vec::new();
if multi {
    let left = gutter + 1;
    let max_name = columns.saturating_sub(left + 3);
    let (mut col, mut row) = (left, legend_row0);
    for (i, s) in series.iter().enumerate() {
        let name = truncate(&s.name, max_name);
        let w = 3 + name.chars().count(); // "── " + name
        if col > left && col + w > columns {
            col = left;
            row += 1;
        }
        legend.push(LegendEntry {
            name,
            color: theme.series(i),
            col,
            row,
        });
        col += w + 3;
    }
}
let legend_rows = legend.last().map_or(0, |e| e.row + 1 - legend_row0);
let total_rows = top + plot_h + 2 + legend_rows;
```

Ordering note: `total_rows` now depends on the legend, so build the legend
before `total_rows`; `label_row` (`compile.rs:550`) is unchanged
(`top + plot_h + 1`). The rasterizer needs no changes — it draws legend
entries at whatever `col`/`row` the Scene carries.

**Step 3: New corpus case** `tests/cases/legend_wrap.json` — five series
whose names overflow one 60-col row... the corpus compiles at DEFAULT size,
so size the names to overflow the CURRENT default width (they must still
wrap at 72 after task 5 — make the combined width > 76 columns):

```json
{
  "data": { "values": [
    {"x": 1, "y": 2, "s": "hot-full-table-scan"},   {"x": 2, "y": 3, "s": "hot-full-table-scan"},
    {"x": 1, "y": 3, "s": "hot-window-projection"}, {"x": 2, "y": 4, "s": "hot-window-projection"},
    {"x": 1, "y": 4, "s": "wide-50-columns"},        {"x": 2, "y": 5, "s": "wide-50-columns"},
    {"x": 1, "y": 5, "s": "sparse-200-columns"},     {"x": 2, "y": 6, "s": "sparse-200-columns"},
    {"x": 1, "y": 6, "s": "ultra-sparse-800"},       {"x": 2, "y": 7, "s": "ultra-sparse-800"}
  ]},
  "mark": "line",
  "title": "legend wraps, never clips",
  "encoding": { "x": {"field": "x"}, "y": {"field": "y"}, "color": {"field": "s"} }
}
```

In the snapshot, verify by hand: all five entries present, wrapped entries'
`row` = first legend row + 1, no entry's `col + 3 + name-width` exceeds
`size.columns`, and `size.rows` grew by the wrap row.

**Step 4:** `make validate`, review pending snapshots. Authorized diffs:
- **Corpus/gallery, titled charts:** one extra blank row — `plot.y` and every
  buffer-absolute row below it shift by +1; `size.rows` +1.
- **Corpus/gallery, multi-series charts:** legend entries move from the
  pre-plot row to below the x labels; `size.rows` grows by the legend row
  count; single-series and bar charts' legends unchanged (empty).
- `multi_series_ansi` (gallery): both effects.

Zero diffs allowed in: tick rows/labels (relative to plot top), mark
geometry, x label columns, error texts. The orchestrator renders a titled
five-series chart in a terminal and eyeballs: blank row under title, legend
under x labels, wrap correct, nothing clipped.

**Step 5:** Commit: `feat(compile): title breathing row; legend flows + wraps below the plot`

---

## Task 4: Palette cap for xy series

**Files:**
- Modify: `crates/benday-core/src/compile.rs` (compile_xy, right after the
  `multi` binding at `compile.rs:461`)
- Modify: `crates/benday-core/src/theme.rs` (doc comment only)
- Create: `crates/benday-core/tests/cases/err_palette_exceeded.json`

**Step 1: The guard** (after `let multi = ...`):

```rust
// Color is the ONLY channel identifying an xy series, so cycling the
// palette would make two series indistinguishable — reject loudly.
// (Categorical bars may cycle: each bar is identified by its x label.)
if series.len() > theme.palette.len() {
    let cf = series_field
        .as_deref()
        .expect("more than one series requires a color field");
    return Err(Error::Data(format!(
        "{} series exceed the {} distinguishable series colors; aggregate or filter \"{cf}\"",
        series.len(),
        theme.palette.len(),
    )));
}
```

**Step 2: theme.rs doc comment** on `series()` (`theme.rs:72`) — keep the
`%` cycle, document the split:

```rust
/// The i-th series color. Wraps past the palette end: safe for categorical
/// BARS (each bar is position-identified by its x label; color is
/// decoration) — xy callers are guarded by compile's palette cap, which
/// rejects charts where color alone must distinguish more series than the
/// palette holds.
```

**Step 3: Corpus case** `err_palette_exceeded.json`: a line chart with 9
single-point series (`s` values `"s1"`..`"s9"`, benday palette holds 8).
Snapshot must read
`ERROR (data): 9 series exceed the 8 distinguishable series colors; aggregate or filter "s"`.

**Step 4:** `make validate`. Authorized diffs: the ONE new corpus snapshot.
Nothing else — no existing case has more than 8 series.

**Step 5:** Commit: `feat(compile): hard error when series exceed palette colors`

---

## Task 5: Default size 72×13

**Files:**
- Modify: `crates/benday-core/src/compile.rs:26-27`
- Modify: `crates/benday-cli/src/main.rs:47-51` (help text)

**Step 1:** `DEFAULT_WIDTH: usize = 72;` `DEFAULT_HEIGHT: usize = 13;`
(rationale comment: 12 row-intervals divide by 2/3/4/6 for the tick search;
72 + gutter stays under 80 columns).

**Step 2:** main.rs flag docs: `(overrides spec.width; default 72)` and
`(overrides spec.height; default 13)`.

**Step 3:** `make validate`, review. Authorized diffs: **every corpus
snapshot** (compiled at defaults): `size`, `plot`, all buffer-absolute
rows/cols, tick sets re-chosen for 12 intervals, bar/label geometry.
**Zero gallery diffs** — the gallery pins explicit sizes. CLI assert_cmd
tests must pass unmodified (they are size-insensitive; if one fails, stop
and report, do not adjust it silently).

**Step 4:** Commit: `feat: default chart size 72x13`

---

## Task 6: Gallery additions, final review, docs

**Files:**
- Modify: `crates/benday-core/tests/gallery.rs`
- Modify: `README.md`, `crates/benday-cli/src/main.rs` EXAMPLES (only if
  they state the old defaults or embed rendered output)

**Step 1: New gallery snapshot** — append to `adversarial_gallery` cases (it
renders at explicit 60×10): name `legend_wrap_5_series`, spec = the
`legend_wrap.json` contents (inline the JSON string, values form).

**Step 2: Full-gallery aesthetic review.** Render every `examples/*.json`
plus the dice chart at the new defaults in a real terminal (color on). The
orchestrator checks, per chart: even tick rhythm, title breathing row,
legend below and lossless, nothing clipped at edges. This is the once-only
per-snapshot review the design doc promises.

**Step 3: README.** Re-render any embedded example output; update stated
defaults; add one line to the feature list: legends wrap below the chart
and never drop series; more series than palette colors is an error.

**Step 4:** `make validate` green; `git status` clean after commit.

**Step 5:** Commit: `docs: chart chrome — gallery legend-wrap snapshot, README refresh`

---

## Execution notes

- Tasks are strictly ordered; 2 and 3 both churn the gallery — the deep
  per-snapshot review happens once, in task 6; tasks 2–5 get the
  authorized-list diff audit described in each task.
- Snapshot acceptance: `cargo insta pending-snapshots` → orchestrator reads
  the diff → `cargo insta accept` only for files on the task's list.
- Every commit must leave `make validate` green; push at the end and check
  CI (which runs exactly `make validate`).
