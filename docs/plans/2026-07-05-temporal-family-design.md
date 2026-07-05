# Temporal family — design

**Date:** 2026-07-05
**Status:** approved
**Scope:** a temporal field type (scale, calendar ticks, ISO parsing) for
line/point/area x, then `timeUnit` bucketing which unlocks temporal bars.
Temporal y, temporal color, and timezones are explicitly OUT.

## Why — and why this reverses recorded doctrine

CLAUDE.md and the bar-family design record "no temporal scales" as doctrine:
SQL owns bucketing (`date_trunc`) and date formatting (`FORMAT_DATE`); benday
owns layout and geometry. That doctrine was written for one workload — SQL
agents querying analytical cubes — and it still holds there. Two things it
does not cover:

- **Pipelines with no SQL in them.** The second real workload is daily
  software maintenance: an agent checks gcloud logs and monitor metrics each
  morning, or pulls a time range from an MCP server, and wants to chart what
  came back. There is no `date_trunc` in that loop. Today the agent must
  bucket and format timestamps itself before benday will draw them.
- **Spacing is geometry.** An ordinal axis gives every period equal width.
  Irregular series — gappy logs, sparse samples, a quarter missing from a
  cube — render as a lie: the gap disappears. SQL can format labels; it
  cannot make an interval occupy proportional space. Positional truth in
  time is layout, which is exactly the territory benday claims.

The reversal is therefore an extension, not a contradiction: SQL still owns
sorting and may still own bucketing when it is present; benday now handles
time when SQL is absent, and renders true positions always. CLAUDE.md and
the `ingest.rs` DATE→ordinal comment get updated in phase 1.

The work ships in two phases against one design: **phase 1** — temporal
scale, calendar ticks, parsing, for line/point/area; **phase 2** —
`timeUnit` bucketing, which is what temporal bars require.

## Spec semantics

- **`"type": "temporal"`** joins the `FieldType` enum. Declared `DATE`,
  `DATETIME`, `TIMESTAMP`, and `TIME` columns map to temporal (previously
  ordinal). Inference promotes an undeclared string column to temporal only
  when EVERY non-null value parses as ISO date/datetime — one bad value and
  the column stays nominal; there is no partial parsing. The precedence
  chain is unchanged: spec `type` > declared column type > inference. An
  explicit `"ordinal"` restores the old evenly-spaced behavior, so the
  reversal stays escapable per chart.
- **`"timeUnit"`** on the x encoding (phase 2): `"year" | "quarter" |
  "month" | "week" | "day" | "hour" | "minute"`. Semantics are calendar
  TRUNCATION — `"month"` maps `2026-06-14T09:12` to `2026-06`, keeping the
  year. Not Vega-Lite's cyclic "all Junes together"; one word, one meaning.
- **Marks.** Line, point, and area place values at true positions in time.
  Bar with temporal x REQUIRES a `timeUnit` (bars need discrete buckets);
  the error names the fix. Temporal y and temporal color: no use case,
  YAGNI, out.

## Time representation and parsing

A temporal value is **one f64: milliseconds since the Unix epoch**, naive.
That is already the currency of the `Linear` scale machinery, so
positioning, extents, and interpolation come free; only tick selection and
labeling are new code.

Accepted input, a deliberate documented subset:

- `2026-07-05` — date
- `2026-07-05T14:30:00`, optional `.123` fraction — datetime
  (a space instead of `T` also accepted; DuckDB and BigQuery emit it)
- either of the above with `Z` or `±hh:mm` — offset applied, then
  discarded; all values land on one comparable axis
- `14:30:00` — time-of-day, anchored to epoch day zero

Anything else fails with the row number, the offending value, and the four
accepted shapes.

**No new dependency.** benday-core keeps its serde-only footprint. A small
`time` module (~150 lines) hand-rolls the parsing and civil-date↔epoch-day
conversion using Howard Hinnant's algorithms. We need perhaps 5% of chrono —
no timezones, no locales, no clock — and calendar math this small is
property-testable: round-trip epoch↔civil across centuries, leap years
pinned in unit tests. `timeUnit` truncation lives in the same module:
truncate on the civil form, convert back to millis.

## Ticks and labels

Nice-number ticks (the 1/2/5 ladder) are wrong for time — 500,000,000 ms is
not a landmark. Temporal scales walk their own ladder of calendar steps:

`1s 5s 15s 30s · 1m 5m 15m 30m · 1h 3h 6h 12h · 1d · 1w · 1mo · 3mo · 1y 2y 5y …`

Selection picks the finest step whose rendered labels fit the plot width
without collision — the same coarsen-until-it-fits philosophy as
`Linear::row_aligned`, fitting label text instead of rows. Ticks land on
true calendar boundaries (the 1st, midnight, Monday), and the domain
expands outward to the enclosing boundaries — the temporal analogue of
nice-number expansion. If even the coarsest sensible step collides, fall
back to first-and-last labels only, mirroring the linear two-endpoint
fallback.

Labels show only what changes at the step; context appears once and at
rollovers (the d3 idiom, suited to width-starved terminals):

- day steps: `Jun 12  Jun 19  Jun 26`
- month steps: `Jan  Apr  Jul`, year at the first tick and at each rollover
- hour steps: `06:00  12:00`, date anchoring the first tick
- quarter steps: `Q2 '26` — the cube workload's native period
- year steps: `2024  2025`

## Compile pipeline

Temporal x rides the quantitative code path in `compile/xy.rs` — values are
f64 millis, so scene geometry, color series-splitting, and rasterization
are untouched. The single branch point is scale construction:
`FieldType::Temporal` selects the calendar scale instead of
`Linear::nice_from`. The Scene IR does not change; a temporal axis is an
axis whose labels came from a different formatter. Existing snapshots must
show ZERO diffs (no current corpus case declares DATE with a line/point
mark — verify before starting, per snapshot discipline).

Phase 2 wires `timeUnit` as a transform stage before aggregation: truncate
each timestamp to its bucket, then the EXISTING aggregate machinery
(`sum`/`mean`/`count`/…) groups by the truncated value. No new aggregation
code. `count` with no y field yields the events-per-hour debug histogram in
one small spec — the raw-gcloud-logs story.

## Errors that teach (error strings are API)

- Unparseable value in a temporal column → row number, offending string,
  the four accepted formats.
- `bar` + temporal x without `timeUnit` → "bars need discrete time buckets;
  add `\"timeUnit\": \"day\"` (or week/month/…) — or use `line`/`point` for
  continuous time."
- `timeUnit` on a non-temporal field → names the type the field resolved to
  and why (which precedence rung decided).

`--help` gains a temporal section with two worked examples: a quarterly
trend line, and raw log timestamps → hourly bar counts.

## Testing

- Property tests on the `time` module: epoch↔civil round-trips across
  centuries; leap-year and month-length edges pinned.
- Spec→scene corpus cases: temporal line (regular and gappy), promoted
  string column, explicit-ordinal escape hatch, each timeUnit, every
  teaching error.
- Gallery snapshots pin the label idiom: day/month/quarter/hour ladders,
  the year-rollover context line, the two-endpoint fallback.
