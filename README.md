# benday

Terminal charts from a Vega-Lite-style JSON spec, drawn in braille dots.
Built for AI agents: pipe query results in, get a chart the human can read
in the transcript.

![Demo: rendering charts from JSON specs in the terminal](assets/demo.gif)

## Install

```sh
cargo install benday
```

Or from source: `git clone https://github.com/fwojciec/benday && cargo install --path benday/crates/benday-cli`.

## Use

Pipe rows in, pass a spec:

```sh
echo '{"columns":[{"name":"day","type":"STRING"},{"name":"n","type":"INT64"}],
       "rows":[["mon",32],["tue",78],["wed",51]]}' \
  | benday --spec '{"mark":"bar","encoding":{"x":{"field":"day"},"y":{"field":"n"}}}'
```

The spec can instead carry its data inline and arrive on stdin by itself.
`benday --help` documents everything: the spec grammar, both stdin shapes,
the bar rules, and worked examples — an agent needs nothing else.

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
    "x":      { "field": "...", "type"?: "quantitative" | "temporal" | "nominal" | "ordinal",
                "timeUnit"?: "year"|"quarter"|"month"|"week"|"day"|"hour"|"minute",  // buckets time
                "bin"?: true | { "maxbins": 15 } | { "step": 10 } },                 // bins quantitative x
    "y":      { "field": "...", "aggregate"?: "sum" | "mean" | "median" | "min" | "max" | "count" },
    "color"?: { "field": "..." }   // series split, or bar grouping
  },
  "title"?: "...", "width"?: 72, "height"?: 13   // plot area, in cells
}
```

Types are inferred from the data when omitted; a declared column type
(`INT64`, `DATE`, …) beats inference, and an explicit spec `"type"` beats
both. Two bar rules follow from the encoding, with no extra flags:

- **Orientation.** Quantitative `x` + categorical `y` draws *horizontal*
  bars — built for rankings: one row per bar, height sized to the category
  count, names in a label column. Categorical `x` + quantitative `y` stays
  vertical.
- **Grouping.** `color` naming a *third* field splits each category into a
  grouped cluster, one bar per value, with a legend. `color` naming the
  category field itself just tints the bars.

Rows chart in arrival order, so `ORDER BY` in the producing query is the
sort. Unknown spec fields are errors that name the fix, never silently
ignored — a silently wrong chart is the one failure a caller can't detect.

Time on the `x` axis is a real scale, not evenly-spaced labels. A `DATE`,
`DATETIME`, `TIMESTAMP`, or `TIME` column — or any string column whose every
value is ISO — charts on a calendar axis: ticks land on week / month / quarter
boundaries and irregular gaps take proportional space. Line, point, and area
plot true positions; bars need a `"timeUnit"` to bucket first (calendar
truncation, so `"month"` maps `2026-06-14` to `2026-06`). `count` with no value
field is an events-per-bucket histogram — the raw-logs story:

```sh
echo '[{"ts":"2026-06-14T08:03:00"},{"ts":"2026-06-14T08:41:00"},{"ts":"2026-06-14T10:12:00"}]' \
  | benday --spec '{"mark":"bar","encoding":{"x":{"field":"ts","timeUnit":"hour"},"y":{"field":"ts","aggregate":"count"}}}'
```

Timestamps without an offset read as UTC civil time; with `Z` or `±hh:mm` they
normalize to UTC. Explicit `"type": "ordinal"` restores evenly-spaced dates.

`bin` on a quantitative `x` draws a histogram — the distribution question a plain
bar can't answer. benday picks nice bins automatically (`true`), or takes a
ceiling (`{"maxbins": 15}`) or an exact width (`{"step": 10}`), counts the rows
per bin, and tiles them as contiguous bars on a linear axis. `y` names the
aggregate: `count` for a frequency histogram, or `mean`/`sum`/… per bin ("mean
latency by payload-size bucket"). Bins are half-open `[edge, next)`; the final
bin is closed, so the maximum value lands in it. Same rule as time: when SQL
already bucketed the values, pass them as an ordinary bar; `bin` is for when
there is no SQL in the loop.

```sh
echo '[{"latency_ms":8},{"latency_ms":23},{"latency_ms":25},{"latency_ms":41},{"latency_ms":52},{"latency_ms":118}]' \
  | benday --spec '{"mark":"bar","encoding":{"x":{"field":"latency_ms","bin":{"step":10}},"y":{"field":"latency_ms","aggregate":"count"}}}'
```

## Data on stdin

With `--spec`/`--spec-file`, stdin carries the data, auto-detected between
two shapes:

- **Columnar envelope** — `{"columns": [...], "rows": [[...]]}`, the shape
  an MCP query tool emits as `structuredContent`. Extra keys are ignored,
  so pipe it straight in; `truncated` and `total_rows` flow to `--meta`.
- **Bare array of row objects** — `[{"day":"mon","n":32}, ...]`.

Declared dates chart on a calendar axis — true spacing, calendar ticks;
`"type": "ordinal"` restores even spacing for periods SQL already bucketed.

## Flags and output

`--marker braille|octant` · `--bar-style dots|blocks` · `--theme
benday|lichtenstein|rotogravure` · `--width`/`--height` · `--no-color` ·
`--meta`

Output is deterministic and color stays on when piped — the transcript is
the display. Errors are JSON on stderr with the fix in the message; exit
`2` = invalid spec, `3` = data doesn't fit the encoding. `--meta` prints
scale domains, resolved series colors, and dropped-row counts to stderr,
so a caller can verify what was drawn without parsing dot art.

## Development

Two-stage pipeline: `compile(spec, data) -> Scene` makes every data- and
layout-dependent decision; `rasterize(scene) -> glyphs` turns the geometry
into cells. A new chart type is compiler work; a new visual style is
rasterizer work. Three test layers: a spec→scene snapshot corpus
(`crates/benday-core/tests/cases/`), rasterizer unit tests, and a rendered
glyph gallery. `make validate` runs exactly what CI runs. Design records
live in `docs/plans/`.

## Status

Works: bars (vertical, horizontal, grouped), histograms (`bin`), line, point,
area, multi-series legends, aggregation, temporal scales (calendar ticks,
`timeUnit` bucketing), type inference, three themes. Planned: stacked bars,
value labels at bar ends, `benday schema` (JSON Schema output). Deliberately
absent: a sort grammar — SQL owns sorting, and owns bucketing when it is
there; benday owns time and value binning when SQL is absent. Negative bars
are a hard error, not a silent miss.

Named for [Ben-Day dots](https://en.wikipedia.org/wiki/Ben_Day_process):
images composed from a raster of small marks, which is what terminal cells
are. MIT. Octant glyph table derived from
[ratatui](https://github.com/ratatui/ratatui) (MIT).
