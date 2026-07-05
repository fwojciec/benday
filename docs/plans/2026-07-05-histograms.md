# Histograms (`bin`) — implementation plan

> **For Claude:** Orchestrator + subagent execution, as in the temporal
> cycle. REQUIRED SUB-SKILL: superpowers:subagent-driven-development — one
> fresh subagent per task, adversarial review after each. The orchestrator
> personally runs `make validate`, audits every snapshot diff against the
> task's authorized list (this cycle authorizes ZERO existing-snapshot
> diffs, all tasks), and renders charts in a real terminal for tasks 4–5.
> Work in a dedicated worktree (superpowers:using-git-worktrees).

**Goal:** `"bin": true | {"maxbins": N} | {"step": w}` on quantitative x
for bars — nice automatic bins, contiguous rects, an edge-ticked linear
axis, `--meta` bin verification, twelve teaching errors.

**Design:** `docs/plans/2026-07-05-histograms-design.md` — the semantic
authority; read it first, in full. Twice Codex-reviewed; every rule
below is pinned there with its rationale. Where plan and design
disagree, STOP and reconcile with the human.

**Architecture:** Bin selection lives beside `nice_num` in `scale.rs`
(pure, sweep-tested). Grammar and spec-level validation in `spec.rs` +
`preflight`. The bar meta detector re-keys on the `Bars` mark's explicit
`direction` (zero-diff refactor) BEFORE the new path lands. The
histogram compile path is a sibling of `compile_bar` producing ordinary
`Bars { direction: Vertical }` whose integer-edge rects tile the plot.
Rasterizer untouched.

**Snapshot referee (STRICT, simplest cycle yet):** ZERO diffs in every
existing corpus and gallery snapshot, all five tasks — this cycle has no
authorized migration. Everything lands as NEW snapshots, each reviewed
line by line before acceptance. New gallery cases pin explicit sizes.

---

## Task 1: Bin selection in `scale.rs`

**Files:**
- Modify: `crates/benday-core/src/scale.rs` (new code beside `nice_num`,
  `scale.rs:96`; `nice_num` and `next_nice` are currently private — the
  new fns live in the same file so nothing needs re-exporting)

**Step 1: Failing tests** (in `scale.rs` `#[cfg(test)]`):

- Auto selection sweep: for `min/max` in a grid of spans (0.001 to 1e9,
  negative-straddling, all-negative) and `target` in 5..=20: every edge
  is an exact multiple of `step`; `step` is 1/2/5×10^k; `lo <= min`,
  `hi >= max`; bin count is within [2, target + 2] (nice rounding may
  add a bin or two).
- Degenerate span: `min == max` (e.g. both 7.3) → treated as
  `min..min + 1.0` (the `Linear::nice_from` guard, `scale.rs:17`) —
  selection returns at least one bin containing the value.
- `maxbins`: count never exceeds N — pin a case where the first nice
  step overshoots and the ladder must coarsen (`next_nice`).
- `step` verbatim: `step: 10` over data 3..97 → `lo = 0, hi = 100,
  n = 10`, edges exactly 0,10,…,100.
- Boundary rule: `bin_index(x, ...)` is half-open `[edge, next)` with
  the FINAL bin closed — `bin_index(hi) == n - 1`, an interior edge
  value lands in the bin to its right.
- Integer cell edges: `cell_edges(n, plot_w)` returns `n + 1`
  monotonically non-decreasing integers from 0 to `plot_w`, computed as
  `round(k/n * plot_w)` — pin `plot_w = 10, n = 4` → `[0, 3, 5, 8, 10]`
  (the Codex counterexample, now tiling correctly).

**Step 2: Run** `cargo test -p benday-core scale` — FAIL (fns missing).

**Step 3: Implement:**

```rust
/// A resolved bin layout: `n` bins of width `step` from `lo`.
/// Every edge is `lo + k*step`; edges are nice numbers unless the
/// caller forced `step`.
#[derive(Debug, Clone, Copy)]
pub struct Bins {
    pub lo: f64,
    pub step: f64,
    pub n: usize,
}

pub fn bins_auto(min: f64, max: f64, target: usize) -> Bins
pub fn bins_maxbins(min: f64, max: f64, n: usize) -> Bins   // coarsen while count > n
pub fn bins_step(min: f64, max: f64, step: f64) -> Bins     // caller width verbatim
impl Bins {
    pub fn hi(&self) -> f64 { self.lo + self.step * self.n as f64 }
    /// Half-open [edge, next); the final bin is closed so x == hi lands in it.
    pub fn index(&self, x: f64) -> usize
}
/// n+1 integer cell edges tiling the plot: round(k/n * plot_w).
pub fn cell_edges(n: usize, plot_w: usize) -> Vec<usize>
```

All three constructors apply the degenerate-span guard FIRST (`if
!(max - min).is_normal() { max = min + 1.0 }`). Callers guarantee
positive/nonzero inputs (task 2 validates); `debug_assert!` them here.

**Step 4: Run** `cargo test -p benday-core scale` — PASS. `make
validate` — zero diffs (nothing wired).

**Step 5: Commit:** `feat(scale): bin selection — nice auto bins, maxbins coarsening, verbatim step, integer cell edges`

---

## Task 2: Grammar + spec-level teaching errors

**Files:**
- Modify: `crates/benday-core/src/spec.rs:78-90` (Channel + BinValue)
- Modify: `crates/benday-core/src/compile/mod.rs:75` (`preflight`),
  error constructors near `:315`
- Create: `crates/benday-core/tests/cases/err_bin_maxbins_and_step.json`,
  `err_bin_maxbins_zero.json`, `err_bin_step_negative.json`,
  `err_bin_on_y.json`, `err_bin_on_color.json`, `err_bin_with_line.json`,
  `err_bin_and_timeunit.json`

**Step 1: Grammar.**

```rust
/// `"bin": true` | `{"maxbins": N}` | `{"step": w}`. `false` is
/// accepted and means absent (Vega-Lite emits it; rejecting would be
/// noise). Numbers stay permissive (f64) so PREFLIGHT can teach —
/// serde must not bounce `maxbins: 0` with a generic type error.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum BinValue {
    Flag(bool),
    Config { maxbins: Option<f64>, step: Option<f64> },
}
```

`Channel` gains `#[serde(default)] pub bin: Option<BinValue>`. NOTE:
`deny_unknown_fields` goes on the Config variant struct — verify with a
test that `{"bin": {"extent": [0,1]}}` errors path-precisely rather
than silently matching `Flag`.

**Step 2: Preflight errors** (resolved types NOT needed — these are
pure spec shape; the type-dependent errors are task 4's):

- design #1 `maxbins` + `step` together; #11 `maxbins` non-integer or
  < 1; #12 `step` not finite-positive
- #5 `bin` on y; #6 `bin` on color; #8 `bin` with non-bar mark;
  #9 `bin` + `timeUnit` on the same channel

Copy the message text from the design's numbered inventory verbatim —
error strings are API. One constructor each in `compile/mod.rs`.

**Step 3: TDD via corpus.** Write the seven `err_*.json` cases first,
run `cargo test -p benday-core --test corpus` — FAIL; implement; PASS;
`cargo insta review` each new snapshot.

**Step 4:** `make validate` — ZERO existing diffs (`bin` is accepted by
serde but `bar` + valid bin isn't routed yet: a valid histogram spec at
this point should hit the existing both-quantitative bar error — add a
TEMPORARY corpus case `bin_not_yet_routed.json` pinning that, deleted in
task 4, so the intermediate state is defined, not accidental).

**Step 5: Commit:** `feat(spec): bin grammar — true/maxbins/step, spec-level teaching errors`

---

## Task 3: Bar meta detector re-keys on direction (zero-diff refactor)

**Files:**
- Modify: `crates/benday-core/src/scene.rs:195-240` (bar meta branch)

**Step 1:** The branch currently detects horizontal bars by
`x_axis.categories.is_none()` and then `.expect`s y categories
(`scene.rs:200-207`) — a histogram scene (vertical, domain-carrying x)
would panic. Re-key: read the `direction` off the scene's
`SceneMark::Bars` (bar scenes have exactly one; `expect` that). Match:

- `Horizontal` → today's horizontal shape, byte-identical.
- `Vertical` + `x_axis.categories: Some` → today's nominal/timeUnit
  shape, byte-identical.
- `Vertical` + categories `None` → `unreachable!("histogram meta lands
  in task 4")` for now.

**Step 2:** `make validate` — ZERO diffs anywhere: this is the entire
proof of the refactor. Also run
`cargo run -q -p benday -- --meta` over one vertical, one horizontal,
one timeUnit example from `examples/` and diff stderr against master's
output (the orchestrator does this personally).

**Step 3: Commit:** `refactor(scene): bar meta detector keys on explicit direction, not category presence`

---

## Task 4: The histogram compile path

**Files:**
- Modify: `crates/benday-core/src/compile/mod.rs` (`bar_route` gains a
  `Histogram` variant; routing near `:455`; type-dependent errors #2,
  #3, #4, #10 near `:315`)
- Modify: `crates/benday-core/src/compile/bars.rs` (new
  `compile_histogram`, sibling of `compile_bar` at `:43`)
- Modify: `crates/benday-core/src/scene.rs:93-100` (XAxis `tick_cols`
  comment says "Empty for bars" — histograms are the first bar scenes
  with tick_cols; comment-only), `:156-168` (`Source` gains
  skip-serialized `bin: Option<BinInfo>`), meta branch (the
  `unreachable!` from task 3 becomes the histogram shape)
- Delete: `tests/cases/bin_not_yet_routed.json` (+ snapshot)
- Create: `tests/cases/hist_auto.json`, `hist_maxbins.json`,
  `hist_step.json`, `hist_empty_bins_count.json` (zero-bar run),
  `hist_gap_mean.json`, `hist_dirty_declared.json` (declared INT64 with
  one dirty string → dropped_rows), `hist_degenerate_span.json`,
  `err_bin_temporal.json`, `err_bin_nominal.json`,
  `err_bin_y_no_aggregate.json`, `err_bin_too_many.json`

**Step 1: Routing.** In `bar_route`: a present-and-active `bin` on x
(Flag(true) or Config) routes `Histogram` BEFORE orientation logic —
a histogram is quantitative×quantitative, which today errors. Inactive
`bin` (`false`) falls through as if absent. Type gate here: x must
resolve Quantitative — Temporal → error #2 (points at `timeUnit`),
Nominal/Ordinal → error #3 (names the rung + the
`"type": "quantitative"` dirty-numeric escape). y must carry an
aggregate → error #4. Errors #2/#3/#4/#10 constructors land here.

**Step 2: `compile_histogram`** (in `bars.rs`):

1. Scan rows: `data::num(xv)` — `None` → `dropped += 1` (legal ONLY
   because resolution was explicit/declared; inferred-quantitative
   means all-numeric by the all-or-nothing rule — assert nothing,
   document the invariant). y values per the existing bar scan
   (count → 1.0).
2. Select bins: `bins_auto(min, max, (plot_w / 4).clamp(5, 20))` /
   `bins_maxbins` / `bins_step` per the spec. If `bins.n > plot_w` →
   error #10 naming the culprit knob (step vs maxbins), the count, the
   width, both fixes.
3. Dense cells: `vec![Vec::new(); bins.n]`, push each row's y into
   `cells[bins.index(xn)]` — every bin exists by construction.
4. Aggregate: the existing `aggregate_cells` with the timeUnit cycle's
   `densified` semantics — empty + `count` → `Some(0.0)`, empty +
   other → `None` (gap at a stable position). Reuse; do NOT fork it.
5. Rects: `let edges = cell_edges(bins.n, plot_w)`; bar `i` is
   `x0 = edges[i] as f64 / plot_w as f64,
   w = (edges[i+1] - edges[i]) as f64 / plot_w as f64` — the
   rasterizer's independent rounding recovers these integers exactly
   (design §Geometry; the `plot_w=10, n=4` Codex case is the pin).
   `None` cells emit no rect (grouped-path `if let Some(v)`
   convention). y normalization identical to `compile_bar`.
6. Axis: `tick_cols = edges[0..n]` (all `< plot_w`; the right domain
   edge gets NO tick glyph — unrepresentable, design §Axis). Labels:
   interior edge values formatted with the linear axis's tick
   formatter, anchored at their edge columns through `place_x_labels`;
   then the RIGHT domain edge as a label right-aligned at the buffer
   end — append it after placement, skipped if it would collide with
   the last survivor.
7. Scene: `x_axis { categories: None, domain: Some([lo, hi]),
   tick_cols, labels }`, `Source.bin = Some(BinInfo { step, domain,
   bins })`, y axis from `Linear::row_aligned(0.0, vmax, …, true)` —
   all existing snapshots stay byte-identical because `bin` is
   skip-serialized.
8. Meta: replace task 3's `unreachable!` with the design's shape —
   `"x": { field, "type": "quantitative", "bin": { step, domain,
   bins } }`, y with aggregate, `direction` omitted (vertical is the
   unmarked case today — keep it that way).

**Step 3: TDD via corpus** — cases first, watch them fail, implement,
review every new snapshot line by line. `hist_step.json` uses
`plot_w`-relevant explicit width so the integer edges are hand-checkable
in the snapshot.

**Step 4:** `make validate` — ZERO existing diffs; the deleted
`bin_not_yet_routed` case is the only removal. Render `hist_auto` and
`hist_empty_bins_count` in a real terminal; LOOK at the silhouette and
the zero-bar run.

**Step 5: Commit:** `feat(compile): histograms — binned x, contiguous integer-edge rects, edge-ticked axis, bin meta`

---

## Task 5: Gallery, README, --help

**Files:**
- Modify: `crates/benday-core/tests/gallery.rs` + fixtures: bell-ish
  histogram, skewed with empty-bin run, `step: 10` latency chart,
  tick-alignment pins at widths 30/50/72 (including one where the last
  interior tick label and the right-aligned domain label nearly meet) —
  ALL explicit sizes, never `examples/*.json`
- Modify: `README.md` (spec grammar block gains `"bin"`; one histogram
  example)
- Modify: `crates/benday-cli/src/main.rs:9` (`EXAMPLES`): bin section —
  the three shapes, one sentence each; a worked latency-distribution
  example; the boundary rule (final bin closed) stated in one line
- Modify: `CLAUDE.md` doctrine bullet: "SQL owns bucketing when
  present" now covers VALUE bucketing too — benday owns `bin` when SQL
  is absent, same clause as `timeUnit`; one-line edit, link the design

**Steps:** gallery TDD (case → rendered snapshot reviewed against the
design's axis rules → accept), docs, `make validate` (zero diffs
outside new gallery snapshots), real-terminal render of every new
gallery case at its pinned size.

**Commit:** `docs: histograms — gallery, README, --help bin section, doctrine touch`

---

## Execution notes for the orchestrator

- Task order is strict: 1 → 2 → 3 → 4 → 5. Task 3 before 4 is
  load-bearing (the meta panic).
- After each task: adversarial subagent review against plan AND design,
  `make validate` run BY YOU, snapshot audit. ZERO existing-snapshot
  diffs authorized this cycle — any existing diff is a defect, full
  stop.
- Error strings are API: copy from the design's twelve-item inventory
  verbatim.
- The design doc wins every disagreement; STOP and reconcile with the
  human if one appears.
