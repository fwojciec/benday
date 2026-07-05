# benday

Terminal charts from a Vega-Lite-style JSON spec, drawn in braille dots.
Named for [Ben-Day dots](https://en.wikipedia.org/wiki/Ben_Day_process) —
images composed from a raster of small marks, which is what your terminal's
cells are.

![Demo: rendering charts from JSON specs in the terminal](assets/demo.gif)

## Why

AI coding agents live in terminals, and terminals can't show images — but
agent work constantly produces tabular data worth *looking* at. benday is
the missing renderer: pipe rows and a tiny spec in, get a chart in the
transcript.

```sh
# rows from a query engine on stdin, a tiny spec via --spec
echo '{"columns":[{"name":"day","type":"STRING"},{"name":"n","type":"INT64"}],
       "rows":[["mon",32],["tue",78],["wed",51]]}' \
  | benday --spec '{"mark":"bar","encoding":{"x":{"field":"day"},"y":{"field":"n"}}}'
```

The spec can also carry its data inline and be piped on its own — see
[Data on stdin](#data-on-stdin) for both flows.

## The spec

A strict subset of Vega-Lite:

```jsonc
{
  "data"?: {                                   // optional — omit to pipe rows in
    "values": [ /* one JSON object per row */ ]           // tidy rows, OR
    // "columns": [ {"name":"day","type":"STRING"} ], "rows": [ ["mon",32] ]  // columnar
  },
  "mark": "bar" | "line" | "point" | "area",
  "encoding": {
    "x":      { "field": "...", "type"?: "quantitative" | "nominal" | "ordinal" },
    "y":      { "field": "...", "aggregate"?: "sum" | "mean" | "median" | "min" | "max" | "count" },
    "color"?: { "field": "..." }   // series split for line/point/area
  },
  "title"?: "...", "width"?: 72, "height"?: 13   // plot area, in cells
}
```

Types are inferred from the data when omitted. Two bar rules follow from the
encoding, with no extra flags:

- **Orientation.** A quantitative `x` with a categorical `y` draws
  *horizontal* bars — they run from the nominal axis toward the quantitative
  one, and the plot height is sized to the category count (so a ranking never
  gets squeezed). Categorical `x` + quantitative `y` stays vertical.
- **Color as grouping.** On a bar chart, `color` naming a *third* field splits
  each category into a grouped cluster (one bar per series). `color` naming the
  category field itself just tints the existing bars.

Flags: `--marker braille|octant`, `--bar-style dots|blocks`, `--theme
benday|lichtenstein|rotogravure`, `--width/--height`, `--no-color`,
`--meta`. Exit codes: `0` ok, `2` invalid spec, `3` data doesn't fit the
encoding; errors are JSON on stderr.

## Data on stdin

The tool is built to sit at the end of a pipe: a query engine emits rows,
benday draws them. When the spec arrives via `--spec` or `--spec-file`, stdin
carries the **data** instead — so `spec.data` becomes optional. (With no spec
flag, stdin is the spec, exactly as before.)

Two stdin shapes are accepted, auto-detected by structure:

- **Columnar envelope** — `{"columns": [...], "rows": [[...]]}`, the shape an
  MCP query tool emits as `structuredContent`. Unknown keys (a `query`
  provenance block, etc.) are ignored, so `structuredContent` pipes straight
  in; the envelope's `truncated` and `total_rows` flow through to `--meta`.
- **Bare array of row objects** — `[{"day":"mon","n":32}, ...]`.

A declared `columns[].type` (case-insensitive, BigQuery and common SQL
spellings) beats type inference exactly where numeric-looking codes and
string-encoded dates would otherwise fool it: `INT64`/`FLOAT64`/`NUMERIC`/…
map to quantitative; `DATE`/`DATETIME`/`TIMESTAMP`/`TIME` map to ordinal for
now (ISO strings sort chronologically — real temporal scales are next cycle);
anything else, including unrecognized type names, falls back to nominal.
Resolution precedence is strict: an explicit spec `"type"` beats a declared
column type beats inference — the spec is the caller's stated intent and
always wins.

With `--meta`, piped data adds a `data` block reporting `source`, `truncated`,
and `total_rows` — provenance the caller can't otherwise see. Inline data
emits no such block (it's the caller's own bytes). The reasoning behind this
spec/data split lives in
[docs/plans/2026-07-04-stdin-data-design.md](docs/plans/2026-07-04-stdin-data-design.md).

## Tradeoffs

Every design choice optimizes for a caller that can't see the output well —
an agent rendering charts for the human reading its transcript.

- **A strict Vega-Lite subset, not full Vega-Lite and not a custom DSL.**
  LLMs already emit Vega-Lite fluently, so specs tend to be valid on the
  first try; a small grammar means everything expressible is guaranteed to
  render. Unknown fields and unsupported channels are *errors with the fix
  in the message*, never silently ignored — a silently wrong chart is the
  one failure mode the caller can't detect.
- **Braille dots everywhere, deliberately.** Round disconnected dots read as
  dithered sub-pixel detail — squint and the chart looks higher-resolution
  than the same grid drawn with blocky glyphs — and it's also the most
  literal Ben-Day rendering possible. The cost: coarser bar caps than block
  characters and one color per cell. `--marker octant` and `--bar-style
  blocks` are the solid-silhouette escape hatches.
- **Agent conventions, inverted where necessary.** Color stays ON when
  piped (the transcript is the display); no TTY sniffing, so output is
  deterministic; `--meta` reports scale domains, series colors, and dropped
  rows so a caller can verify what was drawn without parsing dot art.
- **A CLI, not an MCP server.** Plain process invocation is cheaper and
  more reliable for agents than a protocol wrapper. The core is a pure
  library (`benday-core`, no I/O), so wrapping it later is trivial.
- **Legends wrap below the chart, never drop a series.** With `color`, the
  legend sits under the x labels and wraps onto extra rows rather than
  clipping or omitting entries — the caller must always see every series it
  split by; more series than the theme palette has colors is a hard error,
  not a silent color reuse.

## Architecture

benday is a two-stage pipeline. `compile(spec, &table, opts) -> Scene` resolves every
data- and layout-dependent decision — scale domains, ticks, resolved series
colors, normalized bar/point/line geometry — into a serializable `Scene` IR.
`rasterize(scene, opts) -> Rendered` then maps that normalized geometry to
glyphs and ANSI, knowing nothing about the source data; `render()` composes the
two. The payoff: a new chart type is compiler-only work, a new visual style
(glyphs, colors, bar fills) is rasterizer-only, and the `Scene` between them is
snapshotted as the regression contract.

## Testing

Three layers, from semantic to pixel:

- **Spec→scene corpus** (`crates/benday-core/tests/cases/*.json`) — the
  primary regression contract. Each spec compiles to a `Scene` and the JSON
  IR (domains, ticks, resolved colors, bar/point geometry) is snapshotted —
  layout-aware, glyph-free.
- **Rasterizer unit tests** — the `Scene`→glyph step in isolation.
- **Glyph gallery** (`tests/gallery.rs`) — full rendered output for every
  example plus adversarial specs, at two sizes.

`make validate` runs fmt + clippy + the whole suite; `make snapshots` opens
`cargo insta review`. To add a corpus case: drop a spec in `tests/cases/`,
run the suite, review the pending snapshot, accept. To inspect a scene ad
hoc, use `benday --dump-scene` (unstable: for debugging, format may change
without notice).

## Install

```sh
git clone https://github.com/fwojciec/benday && cd benday
cargo install --path crates/benday-cli
```

## Status

Early, but the foundation is in place: the compile → Scene → rasterize
pipeline, its golden spec→scene corpus, and the glyph-gallery characterization
tests. Works: all four marks, multi-series lines with legends, horizontal bars
(content-sized rankings) and grouped bars (vertical or horizontal),
aggregation, type inference, themes. Planned: temporal scales, `benday schema`
(JSON Schema output), histograms/binning, stacked bars, `layer` composition, a
Claude Code skill file. Negative bars remain unsupported (a hard error, not a
silent miss).

MIT. Octant glyph table derived from
[ratatui](https://github.com/ratatui/ratatui) (MIT).
