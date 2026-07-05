# benday — working notes for Claude

Terminal charts from a strict Vega-Lite-subset JSON spec, built for AI agents
to call. The caller can't see the output well — every design rule below
follows from that.

## The one command

`make validate` — fmt + clippy `-D warnings` + the full test suite. CI runs
exactly this and nothing else. `make snapshots` opens `cargo insta review`.

## Architecture seam

`compile(spec, &table, opts) -> Scene` resolves every data- and
layout-dependent decision; `rasterize(scene, opts) -> Rendered` maps geometry
to glyphs and knows nothing about data or themes. A new chart type is
compiler-only work; a new visual style is rasterizer-only work. The `Scene`
between them is the snapshotted regression contract.

## Snapshot discipline (the referee)

- The spec→scene corpus (`crates/benday-core/tests/cases/*.json`) is the
  primary contract; the glyph gallery (`tests/gallery.rs`) is the behavioral
  referee.
- Default rule is STRICT: a change must produce ZERO diffs in existing
  snapshots unless a per-case diff was explicitly authorized up front. Never
  accept a snapshot diff you cannot explain line by line.
- New gallery cases pin explicit sizes; do not add content-sized charts to
  `examples/*.json` (the 30×6 examples loop would over-ceiling them).

## Contracts that look like bugs (do not "fix")

- **Type precedence** — spec `type` > declared column type > inference —
  lives in ONE place: `resolved_type` in `compile/mod.rs`. But bar
  ORIENTATION routing deliberately uses a *native-typed* inference rung
  (`bar_route`): numeric strings stay categorical for routing, recoverable by
  the coercion rescue. This asymmetry protects the stdin declared-STRING-y
  contract; it is not drift.
- **Rasterizer vertical bar fill** keeps its exact rounding order
  (`round(h*ph)` then fill `ph-level..ph`) — byte-pinned by the gallery.
  Don't unify it with the horizontal branch; rounding is not associative on
  .5 ties.
- **Error strings are API**: agents pattern-match them to self-correct. They
  exist once each (constructors in `compile/mod.rs`); every error names the
  fix. Never silently ignore a spec field.
- **SQL owns sorting, bucketing, and date formatting**; benday owns layout
  and geometry only. First-seen nominal order preserves `ORDER BY` — that IS
  the ranking. No sort grammar, no temporal scales.
- **House style**: braille dots are the default for every mark — don't flip
  defaults to blocks/octants. Themes are named after print processes;
  gradients in OKLCH.

## Docs

`docs/plans/*.md` are dated design/plan records of past cycles — historical
documents, not living docs; don't retro-edit them. The README is the living
surface.
