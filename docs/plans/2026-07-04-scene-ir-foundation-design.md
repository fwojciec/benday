# Scene IR foundation — design

**Date:** 2026-07-04
**Status:** approved
**Scope:** foundation only — validate tooling, characterization tests, Scene IR
refactor, spec→scene golden corpus. No new user-facing chart features. Output
is behavior-identical to v0.1.

## Why

benday's logic currently lives in `render.rs`, which resolves scales, computes
layout, assigns colors, and draws glyphs in the same functions. Every test is
therefore a glyph test, and every failure conflates "the compiler picked the
wrong domain" with "the rasterizer moved a dot." Before adding features
(temporal scales, stdin data, new marks), we invest in the seams that make
iteration fast and reliable:

- a single strict validation command (`make validate`) that local dev and CI
  share verbatim;
- an explicit input→output test contract, sqllogictest-style, hung on an
  intermediate representation rather than on dot patterns;
- an architecture where the two ways we expect to extend the tool — new chart
  types and new visual styles — never multiply each other.

## Architecture

The core crate becomes an explicit two-stage pipeline. The **Scene** is the
load-bearing seam:

```
                     benday-core
  ┌─────────────────────────────┬──────────────────────┐
  │          compile            │      rasterize       │
  │                             │                      │
  spec.rs ─▶ validate ─▶ transform ─▶ scale ─▶ scene.rs ─▶ raster/ ─▶ ansi.rs
  (serde)    (typed      (aggregate)  (existing) (IR +      braille   (existing)
              checks)                            layout)    octant
                                                            blocks
```

- `compile(spec, options) -> Result<Scene>` — everything data- and
  layout-dependent: validation, aggregation, scale resolution, tick placement,
  gutter sizing, label collision handling, color assignment. Pure,
  deterministic, no glyph knowledge. The current `Frame` logic (gutter math,
  tick dedup, x-label placement) moves here and becomes *data* in the Scene
  rather than draw calls.
- `rasterize(scene, options) -> Rendered` — maps normalized geometry through
  the plot rect into dots, stamps text cells, produces the buffer. Marker
  choice (braille/octant) and bar style (dots/blocks) live entirely here.
- `render()` keeps its exact signature — a two-line composition of the above.
  The CLI is unchanged except for one new flag, `--dump-scene`.

Module fates: `scale.rs`, `theme.rs`, `ansi.rs`, `data.rs`, `error.rs` survive
as-is; `raster.rs` becomes the `raster/` backend; `render.rs` is dismantled
into `compile.rs` + `scene.rs` + `raster/mod.rs`. `--meta` output is derived
from the Scene.

A new chart type is compiler-only work (new mark compiles to existing
primitives); a new visual style is rasterizer-only work. Neither touches the
other.

## The Scene IR

A plain serde-serializable struct — resolved facts, no draw methods:

```rust
pub struct Scene {
    pub size: Size,              // total buffer cells (width, height)
    pub plot: Rect,              // plot area in cells (x, y, w, h)
    pub title: Option<Placed<String>>,     // text + col placement
    pub legend: Vec<LegendEntry>,          // name, hex color, row/col
    pub y_axis: Axis,            // domain, step, ticks: Vec<Tick>
    pub x_axis: Axis,            // categories or linear, ticks
    pub marks: Vec<Mark>,        // geometry, normalized to plot
    pub dropped: Dropped,        // row counts for meta
}

pub struct Tick { pub frac: f64, pub label: String, pub cell: u16 }

pub enum Mark {
    Bars   { series: SeriesRef, bars: Vec<Bar> },      // Bar { x0, x1, h } fracs
    Path   { series: SeriesRef, points: Vec<(f64, f64)> },
    Points { series: SeriesRef, points: Vec<(f64, f64)> },
    Fill   { series: SeriesRef, points: Vec<(f64, f64)> }, // area under path
}

pub struct SeriesRef { pub name: Option<String>, pub color: Rgb }
```

Properties the design depends on:

- **Geometry is normalized** to `[0,1]` fractions of the plot rect; only text
  placement is in cells. The same data at a different `--marker` produces an
  identical Scene — marker never leaks into the compiler.
- **Colors are resolved** to concrete `Rgb` (serialized as hex). Theme choices
  are compile-time facts the corpus asserts on. Value-colored bars (gradient
  per bar) mean `Bar` carries its own color override.
- **Ticks carry both** `frac` (semantic position) and `cell` (layout
  decision), so a corpus diff distinguishes "tick moved because the domain
  changed" from "label placement logic changed."
- **`Dropped`** makes silently-skipped rows (unparseable values) a visible,
  snapshot-tested fact rather than a side effect.
- The Scene is a concrete enum of primitives, **not** a trait-object plugin
  system. Extension is by adding a variant. Small, opinionated tool.

Serialization is `serde_json` pretty-printed with stable field order — that
string *is* the snapshot and the `--dump-scene` output. One representation, no
drift.

`--dump-scene` is an explicitly **unstable** debug interface: documented as
such, no format-stability promise. The corpus may evolve the Scene freely.

## Testing strategy

Three layers, each owning a distinct failure class:

1. **Spec→Scene corpus** (the workhorse) — `tests/cases/*.json` spec files; a
   harness test walks the directory, compiles each at fixed 60×10, and
   insta-snapshots the pretty-printed Scene. Error cases live in the same
   corpus: a spec expected to fail snapshots its error (kind + message), so
   the "errors carry the fix" promise is regression-tested too. Initial
   corpus: the four `examples/` specs plus targeted cases per compiler
   behavior — aggregation variants, multi-series color split, tick collision
   at small heights, numeric-string coercion, each validation rejection.
2. **Rasterizer unit tests** — table-driven, no snapshots: braille/octant
   bit-mapping, Bresenham endpoints, eighth-block rounding, cell color
   last-write-wins. Small and stable; these encode intent, not appearance.
3. **Glyph gallery** — insta snapshots of full no-color renders for
   `examples/` plus a handful of corpus cases at a second size (30×6) to catch
   layout at the margins. Deliberately thin: its job is "the picture didn't
   change unexpectedly," not precision.

Failure attribution: a pure visual change diffs layer 3 only; a compiler
change diffs 1 (and possibly 3); a data bug diffs 1. Layer 3 diffing alone
when the rasterizer wasn't touched is the alarm bell.

## Validate tooling

`make validate` is the single source of truth for "green":

```
validate: fmt-check clippy test
fmt-check:  cargo fmt --check
clippy:     cargo clippy --all-targets -- -D warnings
test:       cargo test --workspace
```

CI becomes checkout + toolchain + `make validate`, nothing else. A
`make snapshots` target wraps `cargo insta review` for the accept/reject loop.

Lint ratchet starts modest and tightens over time:
`#![warn(clippy::unwrap_used, clippy::panic)]` in benday-core only — the crate
agents' output depends on shouldn't be able to die impolitely. Promote further
pedantic lints as patterns settle; never turn on pedantic wholesale.

## Milestones

Ordered so the safety net exists before anything moves:

1. **`make validate` + CI switchover** — Makefile, lint ratchet in
   benday-core, CI reduced to `make validate`. Done: CI green running the same
   command as local dev.
2. **Characterization net** — add insta; glyph-gallery snapshots over
   `examples/` + adversarial specs at two sizes, *before any refactor*. Done:
   snapshots committed, `make validate` green.
3. **Scene extraction** — internally split: (a) define `scene.rs` types; (b)
   carve `compile()` out of `render.rs`/`Frame`; (c) carve `rasterize()`; (d)
   reconnect `render()`. Behavior-identical is the hard requirement;
   milestone 2's snapshots are the referee — zero glyph diffs accepted.
4. **Corpus + `--dump-scene`** — harness walking `tests/cases/`, initial ~15
   cases including error cases, CLI flag documented as unstable. Done: corpus
   green, README testing section updated.
5. **Cleanup pass** — delete dead `Frame` code, README architecture blurb.

Execution model: milestones 1, 2, 4, 5 are bounded subagent tasks with
mechanical acceptance checks. Milestone 3 needs vision-holding: the
orchestrator decomposes 3a–3d into sequenced tasks and reviews the Scene type
definitions (3a) before carving starts, since every later task builds on those
types. Snapshots make subagent verification objective — either the glyphs diff
or they don't.

## Out of scope (deliberately)

Temporal scales, data-on-stdin, `benday schema`, new marks, crates.io publish.
Each gets its own planning cycle on top of this foundation. No public
scenegraph contract, no plugin system, no MCP wrapper.
