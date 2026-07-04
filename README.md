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
echo '{
  "data": { "values": [ {"day":"mon","n":32}, {"day":"tue","n":78}, {"day":"wed","n":51} ] },
  "mark": "bar",
  "encoding": { "x": {"field":"day"}, "y": {"field":"n"} }
}' | benday
```

## The spec

A strict subset of Vega-Lite:

```jsonc
{
  "data": { "values": [ /* one JSON object per row */ ] },
  "mark": "bar" | "line" | "point" | "area",
  "encoding": {
    "x":      { "field": "...", "type"?: "quantitative" | "nominal" | "ordinal" },
    "y":      { "field": "...", "aggregate"?: "sum" | "mean" | "median" | "min" | "max" | "count" },
    "color"?: { "field": "..." }   // series split for line/point/area
  },
  "title"?: "...", "width"?: 60, "height"?: 10   // plot area, in cells
}
```

Types are inferred from the data when omitted. Flags: `--marker
braille|octant`, `--bar-style dots|blocks`, `--theme
benday|lichtenstein|rotogravure`, `--width/--height`, `--no-color`,
`--meta`. Exit codes: `0` ok, `2` invalid spec, `3` data doesn't fit the
encoding; errors are JSON on stderr.

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

## Install

```sh
git clone https://github.com/fwojciec/benday && cd benday
cargo install --path crates/benday-cli
```

## Status

Early. Works: all four marks, multi-series lines with legends, aggregation,
type inference, themes. Planned: temporal scales, `benday schema` (JSON
Schema output), histograms/binning, negative and horizontal bars, `layer`
composition, a Claude Code skill file.

MIT. Octant glyph table derived from
[ratatui](https://github.com/ratatui/ratatui) (MIT).
