# Scene IR Foundation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split benday-core into an explicit compile→Scene→rasterize pipeline with a
spec→scene golden corpus, glyph characterization net, and a single `make validate`
command — behavior-identical output throughout.

**Architecture:** `compile(spec, opts) -> Scene` resolves all data- and layout-dependent
decisions into a serializable IR; `rasterize(scene, opts) -> Rendered` maps normalized
geometry to glyphs. `render()` keeps its signature as the composition. Design doc:
`docs/plans/2026-07-04-scene-ir-foundation-design.md` — read it first.

**Tech Stack:** Rust (edition 2021), serde/serde_json, insta (snapshot testing),
make, GitHub Actions.

**Hard rule for every task:** the rendered output of v0.1 must not change. The glyph
snapshots created in Task 2 are the referee. If an EXISTING snapshot diffs during
Tasks 3–6 and you did not intend a visual change, the code is wrong — not the
snapshot; never accept such a diff. NEWLY AUTHORED snapshots (Task 3's smoke test,
Task 6's corpus) are accepted, but only after reviewing each one against what the
output should be.

**Execution notes:**

- Run in a worktree if executing alongside other work: `git worktree add ../benday-scene-ir master`.
- Every task ends with `make validate` green and a commit.
- `cargo insta` CLI is needed for reviewing: `cargo install cargo-insta` (once, on the machine).
- Tasks 3, 4, 5 are sequenced (each builds on the last). Tasks within milestone 3
  must not be parallelized.

---

## Task 1: `make validate` + CI switchover

**Files:**
- Create: `Makefile`
- Modify: `.github/workflows/ci.yml`
- Modify: `crates/benday-core/src/lib.rs` (lint ratchet)

**Step 1: Create the Makefile**

```make
.PHONY: validate fmt clippy test snapshots

validate: fmt clippy test

fmt:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test --workspace

snapshots:
	cargo insta review
```

Note: recipe lines are tabs, not spaces.

**Step 2: Run it**

Run: `make validate`
Expected: all three stages pass (12 tests currently).

**Step 3: Add the lint ratchet to benday-core**

At the very top of `crates/benday-core/src/lib.rs`, before any other items:

```rust
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::panic))]
```

Rationale (put this reasoning in the commit message, not a comment): the crate agents'
output depends on shouldn't be able to die impolitely; tests are exempt.

Ratchet semantics, so nobody trips on it later: `clippy::unwrap_used` does NOT cover
`.expect()` (that is the separate `expect_used` lint, deliberately not enabled).
`expect` with a message stating the invariant is the sanctioned pattern for
provably-unreachable failure. `unreachable!` is likewise not covered.

**Step 4: Fix the one existing production unwrap**

`crates/benday-core/src/raster.rs:116` — `braille_char` ends with
`char::from_u32(0x2800 + u32::from(v)).unwrap()`. The whole braille block
U+2800..=U+28FF is valid scalar values, so convert to the sanctioned pattern:

```rust
char::from_u32(0x2800 + u32::from(v)).expect("U+2800..=U+28FF are valid chars")
```

**Step 5: Run `make validate`**

If clippy flags anything else in non-test lib code, restructure to return `Error`
instead. Expected: green.

**Step 6: Reduce CI to the same command**

Replace the three `run:` steps in `.github/workflows/ci.yml` so the job body is:

```yaml
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: make validate
```

**Step 7: Commit**

```bash
git add Makefile .github/workflows/ci.yml crates/benday-core/src/lib.rs crates/benday-core/src/raster.rs
git commit -m "build: single make validate shared by local dev and CI, lint ratchet in core"
```

Push and confirm CI is green before continuing.

---

## Task 2: Characterization net (glyph gallery snapshots)

This net must exist and be committed BEFORE any refactor code is touched.

**Files:**
- Modify: `crates/benday-core/Cargo.toml` (add insta dev-dependency)
- Create: `crates/benday-core/tests/gallery.rs`
- Generated: `crates/benday-core/tests/snapshots/*.snap` (committed)

**Step 1: Add insta**

In `crates/benday-core/Cargo.toml`:

```toml
[dev-dependencies]
insta = "1"
```

**Step 2: Write the gallery test**

`crates/benday-core/tests/gallery.rs` — snapshots every `examples/` spec at two sizes,
plus adversarial specs, plus one octant and one blocks variant so the alternate
rasterizer paths are protected too. Each snapshot bundles glyphs and meta so meta
regressions are also caught:

```rust
//! Characterization snapshots: the exact rendered output of every example and
//! a set of adversarial specs. These are the referee for the Scene IR
//! refactor — a diff here without an intended visual change means a bug.

use std::fs;
use std::path::Path;

use benday_core::{render, spec::Spec, theme, BarStyle, Marker, RenderOptions};

fn opts(width: usize, height: usize) -> RenderOptions {
    RenderOptions {
        width: Some(width),
        height: Some(height),
        marker: Marker::Braille,
        bar_style: BarStyle::Dots,
        theme: theme::by_name("benday").unwrap(),
        color: false,
    }
}

fn snap(name: &str, spec: &Spec, o: &RenderOptions) {
    let out = render(spec, o).unwrap();
    let bundle = format!(
        "{}\n--- meta ---\n{}",
        out.text,
        serde_json::to_string_pretty(&out.meta).unwrap()
    );
    insta::assert_snapshot!(name.to_string(), bundle);
}

fn parse(json: &str) -> Spec {
    serde_json::from_str(json).unwrap()
}

#[test]
fn examples_gallery() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no example specs found");
    for path in &paths {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let spec: Spec = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        for (w, h) in [(60, 10), (30, 6)] {
            snap(&format!("{stem}_{w}x{h}"), &spec, &opts(w, h));
        }
    }
}

#[test]
fn adversarial_gallery() {
    let cases: &[(&str, &str)] = &[
        (
            "long_labels",
            r#"{"data":{"values":[
                {"c":"extraordinarily long category name","v":4},
                {"c":"another very long one indeed","v":9},
                {"c":"short","v":2}]},
              "mark":"bar","encoding":{"x":{"field":"c"},"y":{"field":"v"}}}"#,
        ),
        (
            "single_point_line",
            r#"{"data":{"values":[{"x":5,"y":3}]},
              "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        ),
        (
            "count_aggregate",
            r#"{"data":{"values":[{"k":"a"},{"k":"a"},{"k":"b"},{"k":"a"},{"k":"c"}]},
              "mark":"bar",
              "encoding":{"x":{"field":"k"},"y":{"field":"k","aggregate":"count"}}}"#,
        ),
        (
            "negative_line",
            r#"{"data":{"values":[{"x":0,"y":-20},{"x":1,"y":15},{"x":2,"y":-5},{"x":3,"y":30}]},
              "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        ),
        (
            "numeric_strings",
            r#"{"data":{"values":[{"m":"a","v":"12"},{"m":"b","v":"7.5"}]},
              "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        ),
        (
            "tick_collision_h7",
            r#"{"data":{"values":[{"m":"a","v":160},{"m":"b","v":40}]},
              "mark":"bar","height":7,
              "encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        ),
    ];
    for (name, json) in cases {
        let spec = parse(json);
        let (w, h) = (60, 10);
        // tick_collision_h7 relies on spec.height; don't override it.
        let o = if *name == "tick_collision_h7" {
            RenderOptions { width: Some(w), height: None, ..opts(w, h) }
        } else {
            opts(w, h)
        };
        snap(name, &spec, &o);
    }
}

#[test]
fn style_variants() {
    let bar = parse(
        r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7},{"m":"mar","v":5}]},
          "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
    );
    snap(
        "bar_blocks",
        &bar,
        &RenderOptions { bar_style: BarStyle::Blocks, ..opts(60, 10) },
    );
    let line = parse(
        r#"{"data":{"values":[{"x":0,"y":1},{"x":1,"y":4},{"x":2,"y":2},{"x":3,"y":6}]},
          "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
    );
    snap(
        "line_octant",
        &line,
        &RenderOptions { marker: Marker::Octant, ..opts(60, 10) },
    );
}

/// Color is ON by default for CLI callers, so the ANSI path needs its own
/// characterization — the no-color gallery cannot see a regression in theme
/// colors for title/legend/axis chrome or per-mark colors. Escape codes make
/// these snapshots ugly to read; their job is diffing, not reading.
#[test]
fn colored_variants() {
    let bar = parse(
        r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7},{"m":"mar","v":5}]},
          "mark":"bar","title":"colored bar",
          "encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
    );
    snap("bar_ansi", &bar, &RenderOptions { color: true, ..opts(60, 10) });
    let lines = parse(
        r#"{"data":{"values":[
            {"m":"a","v":1,"r":"west"},{"m":"b","v":4,"r":"west"},
            {"m":"a","v":2,"r":"east"},{"m":"b","v":3,"r":"east"}]},
          "mark":"line","title":"colored lines",
          "encoding":{"x":{"field":"m"},"y":{"field":"v"},"color":{"field":"r"}}}"#,
    );
    snap("multi_series_ansi", &lines, &RenderOptions { color: true, ..opts(60, 10) });
}
```

**Step 3: Generate and accept the initial snapshots**

Run: `cargo test -p benday-core --test gallery`
Expected: FAIL (new snapshots pending).

Run: `cargo insta accept`
Run: `cargo test -p benday-core --test gallery`
Expected: PASS.

**Step 4: Eyeball every snapshot**

Open each file in `crates/benday-core/tests/snapshots/` and confirm the chart looks
correct (this is the one and only time accepting without scrutiny is forbidden —
these snapshots become the ground truth). Note in the commit message anything odd
you observed but kept (e.g. the known dropped y-tick at height 7).

**Step 5: `make validate`, then commit**

```bash
git add crates/benday-core/Cargo.toml crates/benday-core/tests
git commit -m "test: glyph characterization gallery before Scene IR refactor"
```

---

## Milestone 3: Scene extraction (Tasks 3–5)

Refinement of the design doc's split: the carving is done as vertical slices —
Scene types first, then bars end-to-end through the Scene, then line/point/area —
so `make validate` is green after every task instead of only at the end.

**Behavioral invariants** — the current code's exact decisions, all of which must
be reproduced. Compiler-owned (become Scene data):

- Plot dims: `opts.width.or(spec.width).unwrap_or(60).max(8)`, height `.unwrap_or(10).max(3)`.
- Gutter = max char-count of formatted y-tick labels; buffer is
  `gutter + 1 + plot_w` wide, `title_rows + legend_rows + plot_h + 2` tall.
- Title centered: `gutter + 1 + (plot_w - len)/2` (saturating).
- Legend at row `title_rows`: entries `"── name"` starting at `gutter + 1`, advancing `3 + len + 3`.
- Y tick row = `round((1 - norm(t)) * (plot_h - 1))`; collided rows drop later ticks (first wins).
- X labels: greedy left-to-right, centered `(gutter+1+col) - len/2` (saturating),
  clamped to `width - len`, skipped if it would start before `next_free`; on
  placement `next_free = start + len + 1`.
- Bars: `step = plot_w / n`, `bar_w = clamp(floor(step*0.7), 1, plot_w)`,
  `x0 = min(max(round(center - bar_w/2), 0), plot_w - bar_w)` where `center = (i+0.5)*step`;
  label anchor `min(round(center), plot_w-1)`; truncation to `max(floor(step)-1, 1)` chars with `…`.
- Bar color: `theme.grad(y.norm(v))` per bar, or `theme.series(i)` when color encodes x.
- Y scale: `Linear::nice_from(0, vmax, plot_h.clamp(3,6), true)` for bars;
  `nice_from(ymin, ymax, plot_h.clamp(3,6), mark == Area)` for xy.
- X scale (quant): `nice_from(xmin, xmax, (plot_w/10).clamp(2,7), false)`; nominal: `Linear::indices(n)`.
- Series split: color field ≠ x field; first-seen order; points sorted by x
  (`total_cmp`); single series unnamed → `theme.accent`; multi → `theme.series(i)`.
- Meta JSON: byte-identical shape to today (Task 2 snapshots pin it).

Rasterizer-owned (consume normalized fracs, apply today's rounding):

- Dots bar fill: `level = round(frac_h * plot_h*4)` dot-rows from the bottom over
  dot-columns `x0*2 .. (x0+bar_w)*2`.
- Blocks bar fill: `level = round(frac_h * plot_h*8)`, per-cell
  `fill = level - (plot_h-1-r)*8`, `█` when ≥ 8 else `EIGHTHS[fill]`.
- XY mapping: `px = round(frac_x * (pixel_width-1))`, `py = round(frac_y * (pixel_height-1))`
  where frac_y is already flipped (`1 - norm`) by the compiler — decide and document
  which side flips, then be consistent everywhere.
- Point = 2×2 dot square; area = per-column fill from interpolated top to bottom, then
  the line on top; single-point line = 2 horizontal dots; lines = Bresenham per window.
- Axis chrome: `│`/`┤` in gutter column, `└`/`─`/`┴` on axis row — positions all come
  from the Scene (plot rect, tick rows/cols).

Fraction round-trips are exact: cell-integer decisions stored as `k/plot_w` fractions
recover `k` via `.round()` after multiplying back. Always `.round()`, never truncate.

---

## Task 3: Scene types + serialization

**Files:**
- Create: `crates/benday-core/src/scene.rs`
- Modify: `crates/benday-core/src/lib.rs` (add `pub mod scene;`)
- Modify: `crates/benday-core/src/raster.rs` (add `Serialize` for `Rgb` as hex)

**Step 1: Rgb serializes as hex**

In `raster.rs`:

```rust
impl serde::Serialize for Rgb {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}
```

Add `serde = { version = "1", features = ["derive"] }` is already a dependency; no change needed.

**Step 2: Write `scene.rs`**

```rust
//! The Scene: benday's intermediate representation. `compile()` resolves a
//! spec against its data into a Scene — every data- and layout-dependent
//! decision made, geometry normalized to the plot rect — and `rasterize()`
//! turns a Scene into glyphs. The serialized form is the golden-corpus
//! snapshot and the `--dump-scene` output; it is explicitly unstable.

use serde::Serialize;

use crate::raster::Rgb;
use crate::spec::Aggregate;

#[derive(Serialize)]
pub struct Scene {
    pub size: Size,
    pub plot: Rect,
    /// Resolved theme colors for non-mark elements. Colors are compile-time
    /// facts everywhere — the rasterizer never sees a Theme.
    pub chrome: Chrome,
    pub title: Option<Placed>,
    pub legend: Vec<LegendEntry>,
    pub y_axis: YAxis,
    pub x_axis: XAxis,
    pub marks: Vec<SceneMark>,
    pub dropped_rows: usize,
    /// Provenance for --meta output.
    pub source: Source,
}

/// Colors for axes/labels (`axis`) and the title (`title`). Legend swatches
/// carry their own color per entry; legend NAME text uses `axis`.
#[derive(Serialize)]
pub struct Chrome { pub axis: Rgb, pub title: Rgb }

#[derive(Serialize)]
pub struct Size { pub columns: usize, pub rows: usize }

#[derive(Serialize)]
pub struct Rect { pub x: usize, pub y: usize, pub w: usize, pub h: usize }

/// Text plus its resolved starting column (buffer-absolute) and row.
#[derive(Serialize)]
pub struct Placed { pub text: String, pub col: usize, pub row: usize }

#[derive(Serialize)]
pub struct LegendEntry { pub name: String, pub color: Rgb, pub col: usize, pub row: usize }

#[derive(Serialize)]
pub struct YAxis {
    pub domain: [f64; 2],
    pub step: f64,
    /// Deduped, in draw order. `row` is buffer-absolute.
    pub ticks: Vec<YTick>,
}

#[derive(Serialize)]
pub struct YTick { pub value: f64, pub frac: f64, pub label: String, pub row: usize }

#[derive(Serialize)]
pub struct XAxis {
    /// Nominal x: resolved category order. Quantitative: None.
    pub categories: Option<Vec<String>>,
    pub domain: Option<[f64; 2]>,
    /// Columns (plot-relative) that get a '┴' glyph. Empty for bars.
    pub tick_cols: Vec<usize>,
    /// Labels that survived greedy placement; `col` is the buffer-absolute
    /// start column. Dropped labels simply don't appear — visible in diffs.
    pub labels: Vec<Placed>,
}

#[derive(Serialize)]
pub struct SeriesRef { pub name: Option<String>, pub color: Rgb }

#[derive(Serialize)]
pub enum SceneMark {
    Bars {
        /// One entry per category, in category order.
        bars: Vec<Bar>,
    },
    Path { series: SeriesRef, points: Vec<[f64; 2]> },
    Points { series: SeriesRef, points: Vec<[f64; 2]> },
    /// Area: fill under the path plus the path itself.
    Fill { series: SeriesRef, points: Vec<[f64; 2]> },
}

#[derive(Serialize)]
pub struct Bar {
    /// Left edge and width as fractions of plot width (exact multiples of 1/plot_w).
    pub x0: f64,
    pub w: f64,
    /// Height as a fraction of plot height (y.norm of the aggregated value).
    pub h: f64,
    pub color: Rgb,
}

#[derive(Serialize)]
pub struct Source {
    pub mark: crate::spec::Mark,
    pub x_field: String,
    pub y_field: String,
    pub aggregate: Option<Aggregate>,
    /// Points-per-series counts etc. needed to reproduce --meta exactly.
    pub series_points: Vec<usize>,
}

impl Scene {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scene serialization is infallible")
    }

    /// The --meta payload. Must reproduce the pre-refactor format exactly.
    pub fn meta(&self) -> serde_json::Value {
        todo!("implemented in Task 4/5 per mark type")
    }
}
```

Field-order note: serde emits fields in declaration order — that IS the stable order;
don't reorder fields casually later.

Adjust freely where the current code forces it (e.g. `Points` for xy marks are stored
flipped or not — pick when implementing), but keep the shape: normalized geometry,
cell-space text placement, resolved colors, no glyph knowledge. `spec::Mark` and
`Aggregate` already derive `Serialize`.

**Step 3: Serialization smoke test**

At the bottom of `scene.rs`, a unit test that builds a tiny Scene by hand and
insta-snapshots `to_json()` (inline snapshot is fine). Run it, accept, verify.

**Step 4: `make validate`, commit**

```bash
git add crates/benday-core/src/scene.rs crates/benday-core/src/lib.rs crates/benday-core/src/raster.rs
git commit -m "feat(core): Scene IR types with stable JSON serialization"
```

---

## Task 4: Bars through the Scene

**Files:**
- Create: `crates/benday-core/src/compile.rs` (`compile()` entry + bar path)
- Create: `crates/benday-core/src/raster/mod.rs` (move `raster.rs` here; add `rasterize()`)
- Modify: `crates/benday-core/src/render.rs` (bar path delegates; Frame stays for xy)
- Modify: `crates/benday-core/src/lib.rs`

**Step 1: Introduce `CompileOptions` and `compile()`**

```rust
pub struct CompileOptions {
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub theme: Theme,
}

pub fn compile(spec: &Spec, opts: &CompileOptions) -> Result<Scene, Error> { ... }
```

Move `validate()`, `aggregate()`, `truncate()`, and the grouping/scale/layout halves
of `render_bar` into `compile.rs`. The layout math currently inside `Frame::new`,
`draw_y_axis`, and `draw_x_axis` (gutter width, tick rows + dedupe, greedy label
placement) is re-expressed here as Scene data — follow the invariants list above.

**Step 2: Move the canvas module, then implement `rasterize()` for `SceneMark::Bars`**

First the file move, so `mod raster;` resolves to the directory form:

```bash
mkdir crates/benday-core/src/raster
git mv crates/benday-core/src/raster.rs crates/benday-core/src/raster/mod.rs
```

(Consider splitting the canvas into `raster/canvas.rs` re-exported from `mod.rs` if
`mod.rs` gets crowded — optional.)

Then `rasterize(scene: &Scene, opts: &RasterOptions) -> Rendered` where

```rust
pub struct RasterOptions { pub marker: Marker, pub bar_style: BarStyle, pub color: bool }
```

Note the theme is NOT here: every color the rasterizer stamps comes from the Scene —
`scene.chrome.axis` for axes/labels/legend names, `scene.chrome.title` for the title,
per-entry swatch colors, per-bar/series mark colors.

It stamps: title, legend, y-axis chrome + labels (from scene rows), bar fills
(dots via PixelCanvas or blocks via EIGHTHS, recovering cell integers with
`.round()`), x-axis chrome + labels. Implement `Scene::meta()` for bars.

**Step 3: Rewire `render()` for bars only — WITHOUT dropping preflight for xy marks**

`render()` currently runs `validate()` + the `check_field` calls for every mark
before dispatch. During this task the xy path still goes through the old `render_xy`,
so keep a shared preflight both paths use:

```rust
pub fn render(spec: &Spec, opts: &RenderOptions) -> Result<Rendered, Error> {
    match spec.mark {
        Mark::Bar => {
            let scene = compile::compile(spec, &copts)?; // compile() owns preflight
            Ok(raster::rasterize(&scene, &ropts))
        }
        Mark::Line | Mark::Point | Mark::Area => {
            compile::preflight(spec)?; // validate() + check_field, extracted
            render_xy(spec, opts, plot_w, plot_h)
        }
    }
}
```

`compile()` calls the same `preflight()` internally. The duplication for the xy arm
is temporary scaffolding; Task 5 deletes it. The existing unit tests
(`aggregate_on_x_is_rejected` etc.) prove preflight still fires for both arms.

**Step 4: The referee**

Run: `cargo test -p benday-core --test gallery`
Expected: PASS with ZERO diffs — including `bar_ansi` (chrome colors) and
`bar_blocks`. If any snapshot diffs, fix the code until it doesn't. Do not accept.

**Step 5: `make validate`, commit**

`-am` misses new files; stage explicitly:

```bash
git add crates/benday-core/src
git commit -m "refactor(core): bar marks compile to Scene and rasterize from it"
```

---

## Task 5: Line/point/area through the Scene, delete Frame

**Files:**
- Modify: `crates/benday-core/src/compile.rs` (xy path)
- Modify: `crates/benday-core/src/raster/mod.rs` (Path/Points/Fill rasterization)
- Delete: the remains of `crates/benday-core/src/render.rs`'s Frame and render_xy
  (keep `render()`, `RenderOptions`, `BarStyle`, `Rendered`, and the module tests)

**Step 1: Compile xy marks to Scene**

Series split, type inference, sorting, domains, x tick/label layout — all per the
invariants list. Points stored as normalized `[frac_x, frac_y]` pairs.

**Step 2: Rasterize Path/Points/Fill**

Bresenham, 2×2 points, area column fill, single-point special case — recover today's
pixel coordinates exactly (`round(frac * (pixel_dim - 1))`).

**Step 3: Finish `Scene::meta()`** for xy marks (series names/colors/point counts).

**Step 4: `render()` is now two lines for every mark.** Delete `Frame`, `render_bar`,
`render_xy`, and the temporary direct `preflight()` call in `render()` (it lives on
inside `compile()`). Keep the existing unit tests in `render.rs` (they exercise the
public `render()` and must pass unchanged — including the validation-error tests,
which now prove preflight fires via `compile()`).

**Step 5: The referee**

Run: `cargo test -p benday-core --test gallery`
Expected: PASS, zero diffs, including `line_octant`, `bar_blocks`, and both
`*_ansi` colored snapshots.

**Step 6: `make validate`, commit**

```bash
git commit -am "refactor(core): all marks through Scene; Frame deleted; render() is compile+rasterize"
```

---

## Task 6: Golden corpus + `--dump-scene`

**Files:**
- Create: `crates/benday-core/tests/corpus.rs`
- Create: `crates/benday-core/tests/cases/*.json` (list below)
- Modify: `crates/benday-cli/src/main.rs` (flag)
- Modify: `README.md` (testing + dump-scene blurb)

**Step 1: The harness**

```rust
//! Spec→Scene golden corpus. Each tests/cases/*.json is compiled at the
//! default size; the Scene JSON (or the error) is the snapshot. This is the
//! primary regression contract — semantic, layout-aware, glyph-free.

use std::fs;
use std::path::Path;

use benday_core::compile::{compile, CompileOptions};
use benday_core::{spec::Spec, theme};

#[test]
fn corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 15, "corpus shrank: {} cases", paths.len());
    let opts = CompileOptions {
        width: None,
        height: None,
        theme: theme::by_name("benday").unwrap(),
    };
    for path in &paths {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let spec: Spec = serde_json::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{stem}: case must be a parseable spec: {e}"));
        let snapshot = match compile(&spec, &opts) {
            Ok(scene) => scene.to_json(),
            Err(e) => format!("ERROR ({}): {}", e.kind(), e),
        };
        insta::assert_snapshot!(format!("case__{stem}"), snapshot);
    }
}
```

**Step 2: The initial cases** (each a complete spec file; small data, 2–6 rows unless noted)

Happy path: `bar_simple`, `bar_count`, `bar_median`, `bar_color_by_x` (palette not
gradient), `bar_numeric_strings`, `line_multi_series` (color field ≠ x),
`line_nominal_x`, `line_single_point`, `line_negative`, `point_basic`, `area_basic`
(domain includes zero), `long_labels` (truncation + label drops visible in scene),
`tick_collision_h7` (`"height": 7` — pins the known dropped-tick behavior).

Error path: `err_aggregate_on_x`, `err_bar_color_group`, `err_bar_quant_x`,
`err_bar_negative`, `err_missing_field`, `err_empty_data`, `err_categorical_y`.

Reuse the data from the Task 2 adversarial specs where names overlap.

**Step 3: Generate, REVIEW EACH SCENE CAREFULLY, accept**

This is authoring ground truth, same bar as Task 2 Step 4: check domains, tick labels,
colors, bar fractions against what the chart should be — don't accept on vibes.

**Step 4: `--dump-scene` flag**

In `main.rs`: add

```rust
    /// Print the compiled scene as JSON instead of rendering (UNSTABLE:
    /// debugging interface, format changes without notice)
    #[arg(long)]
    dump_scene: bool,
```

and before the `render()` call:

```rust
    if cli.dump_scene {
        return match benday_core::compile::compile(&spec, &copts) {
            Ok(scene) => {
                println!("{}", scene.to_json());
                ExitCode::SUCCESS
            }
            Err(e) => {
                let code = if e.kind() == "spec" { 2 } else { 3 };
                fail(e.kind(), &e.to_string(), code)
            }
        };
    }
```

(Build `copts` once and share it with the render path.)

**Step 5: Verify by hand**

Run: `cargo build && echo '{"data":{"values":[{"m":"a","v":3}]},"mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}' | ./target/debug/benday --dump-scene`
Expected: pretty Scene JSON, exit 0.

**Step 6: README**

Add a short "Testing" section: the three layers, `make validate`, `make snapshots`,
how to add a corpus case (drop a spec in `tests/cases/`, run, review, accept). Mention
`--dump-scene` as unstable. Keep it to ~15 lines — the README stays minimal.

**Step 7: `make validate`, commit**

```bash
git add crates/benday-core/tests crates/benday-cli/src/main.rs README.md
git commit -m "test: spec-to-scene golden corpus; feat(cli): unstable --dump-scene"
```

---

## Task 7: Cleanup pass

**Files:**
- Modify: `README.md` (architecture blurb: compile → Scene → rasterize, one paragraph)
- Review: `crates/benday-core/src/` for dead code (`cargo clippy` should already catch it)

**Step 1:** Grep for leftovers: `rg "Frame|render_bar|render_xy" crates/` — expect no hits
outside comments/history.

**Step 2:** README architecture paragraph + refresh the roadmap section (foundation done).

**Step 3:** `make validate`, final commit:

```bash
git commit -am "docs: architecture notes; foundation milestone complete"
```

Push, confirm CI green. Foundation done — feature work (temporal scales, stdin data)
gets its own plan on top of this.
