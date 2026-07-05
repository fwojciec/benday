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
  YAGNI, out — but "out" means REJECTED WITH A TEACHING ERROR, not left to
  fall through. Today a temporal y would hit the generic "holds categorical
  values" message (`xy.rs` y-gate) and a temporal color would silently
  explode into one series per timestamp; both get explicit errors naming
  the resolved type and the fix (aggregate the time field, or put time
  on x).

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

Values WITHOUT an offset are read as UTC civil time; values WITH one are
normalized to UTC. A column mixing the two therefore compares correctly
only if its naive values really are UTC — benday cannot know, so `--help`
documents the convention instead of guessing. Truncation and tick
boundaries are computed in UTC.

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

`compile/xy.rs` today asks `xt == Quantitative` at four sites: row parsing
(numeric vs interned category), the ordinal category sort, x-scale
selection, and the axis/domain output. Temporal is CONTINUOUS at every one
of those forks, so the change is a two-way classification — continuous
(quantitative | temporal) vs categorical — threaded through all four,
not a scale swap at one. Within the continuous arm, temporal differs in
exactly three places: the value reader (parse ISO instead of `data::num`),
the scale (calendar ladder instead of `Linear::nice_from`), and the label
formatter. Geometry, color series-splitting, and rasterization are
untouched.

**Scene IR and `--meta`.** `Scene::meta()` currently derives the reported
x type from `categories.is_none() → "quantitative"` — a temporal axis
would lie to the very API agents use to verify their chart. `Source` gains
an optional resolved-x-type field, `skip_serializing_if = None` (existing
scene.rs precedent), so every current snapshot stays byte-identical.
`--meta` then reports `"type": "temporal"` and the x domain as ISO strings
rather than raw millis.

**Snapshot migration, authorized up front.** One corpus case —
`declared_date_ordinal.json`, a line chart with a declared DATE x, the
only date-typed case — pins today's ordinal behavior (categorical labels,
equal spacing). Phase 1 changes it BY DESIGN: temporal scale, true
spacing, calendar ticks. That diff is authorized here, must be audited
line by line, and the case is renamed `declared_date_temporal` since its
name encodes the reversed doctrine. Every other existing snapshot must
show zero diffs. The explicit-ordinal escape hatch gets a NEW case
pinning the old behavior under `"type": "ordinal"`.

Phase 2 wires `timeUnit` as a transform stage before aggregation, and the
bucket representation is chosen to ride the existing categorical bar path
untouched: truncation emits a CANONICAL ISO-PREFIX LABEL per bucket
(`2026`, `2026-Q2`, `2026-06`, `2026-06-14`, `2026-06-14 09h`, …) —
zero-padded, so the scanner's text interning groups correctly and
`sort_cats`' lexical order IS chronological. After the scan, the bucket
list is DENSIFIED: every calendar bucket between min and max is inserted
as an empty cell. Empty cells then carry two scoped rules, because the
plain vertical path currently ASSERTS every scanned cell is Some
(`bars.rs` "a scanned category has values" — densify breaks that
invariant by construction):

- `count` of an empty bucket is `Some(0.0)` — count of an empty set is
  well-defined — via an aggregation variant used ONLY by the densified
  temporal path, so existing grouped-bar gap behavior is untouched.
- Every other aggregate stays `None` (mean of nothing is undefined), and
  the plain vertical path handles it the way the grouped path already
  does (`if let Some(v)`): no bar glyph, but the bucket keeps its
  position and tick — a gap at a stable position.

A quiet hour therefore shows as a zero bar under `count` and as a gap
under other aggregates, and non-timeUnit bars keep their invariant
exactly as is. `count` with no y field yields the events-per-hour debug
histogram in one small spec — the raw-gcloud-logs story.

## Errors that teach (error strings are API)

- Unparseable value in a temporal column → row number, offending string,
  the four accepted formats.
- `bar` + temporal x without `timeUnit` → "bars need discrete time buckets;
  add `\"timeUnit\": \"day\"` (or week/month/…) — or use `line`/`point` for
  continuous time."
- `timeUnit` on a non-temporal field → names the type the field resolved to
  and why (which precedence rung decided).
- Temporal y → names the resolved type and the fix (aggregate it, or put
  time on x) instead of today's generic "holds categorical values".
- Temporal color → rejected before it becomes one series per timestamp;
  the error suggests a categorical field or a timeUnit-bucketed phase-2
  alternative.

`--help` gains a temporal section with two worked examples: a quarterly
trend line, and raw log timestamps → hourly bar counts.

## Testing

- The `time` module is verified by EXHAUSTIVE round-trip, not property
  testing: iterate every civil day 1600–2400 (~292k iterations, trivial in
  a unit test), assert epoch↔civil round-trips, pin leap-year and
  month-length edges. Deterministic, stronger than sampling, and keeps
  even dev-dependencies unchanged — "no new dependency" holds for both the
  published crate and the workspace.
- Spec→scene corpus cases: temporal line (regular and gappy), promoted
  string column, explicit-ordinal escape hatch, each timeUnit, every
  teaching error (including temporal y and temporal color).
- The migrated `declared_date_temporal` case (née `declared_date_ordinal`)
  is the one authorized diff; its new snapshot is audited line by line.
- Gallery snapshots pin the label idiom: day/month/quarter/hour ladders,
  the year-rollover context line, the two-endpoint fallback, and a
  densified hourly count with a quiet-hours zero run.
