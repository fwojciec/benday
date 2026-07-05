# Temporal family — implementation plan

> **For Claude:** Orchestrator + subagent execution, as in the bar-family
> cycle. REQUIRED SUB-SKILL: superpowers:subagent-driven-development — one
> fresh subagent per task, adversarial review after each. The orchestrator
> personally runs `make validate`, audits every snapshot diff against the
> task's authorized list, and renders charts in a real terminal for tasks
> 3–5. Work in a dedicated worktree (superpowers:using-git-worktrees).

**Goal:** A temporal field type — ISO parsing, true positions in time,
calendar ticks — for line/point/area, then `timeUnit` bucketing which
unlocks temporal bars.

**Design:** `docs/plans/2026-07-05-temporal-family-design.md` — read it
first, in full. It records WHY this reverses the "no temporal scales"
doctrine and pins every semantic decision referenced below.

**Architecture:** One new pure module (`time.rs`: civil-date math, ISO
parsing, calendar ticks, truncation) with zero dependencies; a continuous-x
classification threaded through `compile/xy.rs`; one optional
`skip_serializing_if` field on `Source`; the timeUnit transform rides the
existing bar scanner by emitting canonical ISO-prefix bucket labels.
Rasterizer untouched.

**Phases:** Tasks 1–3 are phase 1 (temporal scale). Tasks 4–5 are phase 2
(timeUnit). Phase 1 must land green before phase 2 starts.

**Snapshot referee (STRICT):** zero diffs in existing gallery snapshots,
all tasks. Exactly ONE existing-corpus diff is authorized, in task 3:
`declared_date_ordinal` migrates to a temporal axis and is renamed. Every
other change lands as NEW snapshots, each reviewed line by line before
acceptance. New gallery cases pin explicit sizes (CLAUDE.md: never add
content-sized charts to `examples/*.json`).

---

## Task 1: `time` module — civil math and ISO parsing

**Files:**
- Create: `crates/benday-core/src/time.rs`
- Modify: `crates/benday-core/src/lib.rs:8-18` (add `mod time;`)

**Step 1: Write the failing tests first** (in `time.rs` `#[cfg(test)]`):

- Exhaustive round-trip: for every day count `z` from
  `days_from_civil(1600, 1, 1)` to `days_from_civil(2400, 12, 31)`
  (~292k iterations), `civil_from_days(z)` then `days_from_civil` returns
  `z`, and consecutive `z` yield calendar-consecutive dates per a
  reference `days_in_month(y, m)` (leap rule: `y % 4 == 0 && (y % 100 !=
  0 || y % 400 == 0)`).
- Pinned edges: `2000-02-29` parses, `1900-02-29` does not, `2024-02-29`
  parses, `2026-02-29` does not, `2024-01-31` vs `2024-04-31` (rejected).
- Parse table, each shape → expected millis:
  `"2026-07-05"`, `"2026-07-05T14:30:00"`, `"2026-07-05 14:30:00"`
  (space separator — DuckDB/BigQuery emit it), `"2026-07-05T14:30:00.123"`,
  `"2026-07-05T14:30:00Z"`, `"2026-07-05T12:30:00-02:00"` (equals the `Z`
  case at 14:30), `"14:30:00"` (epoch day zero).
- Rejects: `"2026/07/05"`, `"07-05-2026"`, `"2026-7-5"` (no zero-pad),
  `"2026-13-01"`, `"2026-07-05T25:00:00"`, `"hello"`, `""`.

**Step 2: Run** `cargo test -p benday-core time` — expected: FAIL
(module functions not defined).

**Step 3: Implement.** Howard Hinnant's `days_from_civil` /
`civil_from_days` algorithms, verbatim shape:

```rust
/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;                          // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;           // [0, 146096]
    era * 146097 + doe as i64 - 719468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u64, u64) { /* inverse, Hinnant */ }

/// Parse one temporal value to milliseconds since the Unix epoch (naive
/// UTC; an explicit offset is applied, then discarded). None = not temporal.
pub(crate) fn parse_temporal(s: &str) -> Option<f64>
```

Parsing is strict on the four documented shapes: zero-padded fields,
month/day/hour/minute/second range-checked (day against
`days_in_month`), fraction up to 3 digits, offset `Z` or `±hh:mm`.
Time-only anchors to epoch day zero. No timezone database, no locale,
no clock — this module never calls `now()`.

**Step 4: Run** `cargo test -p benday-core time` — expected: PASS.
Then `make validate` — zero snapshot diffs (nothing is wired yet).

**Step 5: Commit:** `feat(time): civil-date math and strict ISO parsing, exhaustively round-tripped`

---

## Task 2: Calendar ticks — the ladder, boundary alignment, context+delta labels

**Files:**
- Modify: `crates/benday-core/src/time.rs` (all of it lives here;
  `scale.rs` stays untouched — `Linear` already does positioning)

**Step 1: Failing tests.** Pin whole `temporal_axis` outputs — tick
positions as ISO strings plus exact labels — for these ranges at
`plot_w = 72`:

- 3 months of daily data → week steps: `Jun 1 '26`, `Jun 8`, `Jun 15`, …
- 2 years of monthly data → quarter steps: `Q1 '25`, `Q2`, … with `Q1 '26`
  at the year rollover
- 36 hours of minute data → 6h steps: `Jun 14 06:00`, `12:00`, `18:00`,
  `Jun 15 00:00`, …
- 6 years → year steps: `2024`, `2025`, …
- Same ranges at `plot_w = 30` → the ladder coarsens; at `plot_w = 12` →
  first-and-last fallback.
- Domain expansion: returned domain is the enclosing boundaries (a series
  `Jun 3..Jun 27` at week steps gets domain `Jun 1..Jun 29` — the
  surrounding Mondays).

**Step 2: Run** `cargo test -p benday-core temporal_axis` — FAIL.

**Step 3: Implement.**

```rust
pub(crate) struct TemporalAxis {
    pub domain: [f64; 2],           // expanded to enclosing boundaries, ms
    pub ticks: Vec<(f64, String)>,  // (position ms, label)
}
pub(crate) fn temporal_axis(min_ms: f64, max_ms: f64, plot_w: usize) -> TemporalAxis
```

The ladder, finest to coarsest — each entry knows how to floor a
timestamp to its boundary and step to the next:

`1s 5s 15s 30s · 1m 5m 15m 30m · 1h 3h 6h 12h · 1d · 1w (Monday) ·
1mo · 3mo · 1y 2y 5y 10y …` (years continue up the 1/2/5 ladder).

Selection: walk finest→coarsest; generate ticks (floor `min` to the
boundary, step until past `max`); format labels; ACCEPT the first step
where the labels fit `plot_w` without collision under the same greedy
rule `place_x_labels` uses — centered on their column, non-overlapping,
one column of separation. If even the coarsest step collides, return
first-and-last only (the linear two-endpoint fallback's temporal twin).

Label formatter (context + delta — context at the first tick and at
each rollover, delta elsewhere):

| step      | delta        | first / rollover        |
|-----------|--------------|-------------------------|
| seconds   | `14:30:05`   | `Jun 14 14:30:05`       |
| minutes   | `14:30`      | `Jun 14 14:30`          |
| hours     | `06:00`      | `Jun 14 06:00` (new day)|
| days/weeks| `Jun 12`     | `Jun 12 '26` (new year) |
| months    | `Jun`        | `Jun '26` (new year)    |
| quarters  | `Q2`         | `Q1 '26` (new year)     |
| years     | `2026`       | —                       |

Month/quarter boundaries step on the civil form via task 1's functions,
then convert back to millis; day-and-finer steps are fixed-width in ms.

**Step 4: Run** `cargo test -p benday-core` — PASS. `make validate` —
zero diffs (still unwired).

**Step 5: Commit:** `feat(time): calendar tick ladder with context+delta labels`

---

## Task 3: Temporal wiring — FieldType, xy continuous path, meta, teaching errors

The cross-cutting task; this is where the doctrine reverses and the ONE
authorized snapshot migration happens.

**Files:**
- Modify: `crates/benday-core/src/spec.rs:91-96` (FieldType)
- Modify: `crates/benday-core/src/ingest.rs:93-101` (mapping + comment),
  `:449` (test expectation)
- Modify: `crates/benday-core/src/data.rs:47-66` (`infer_type` promotion)
- Modify: `crates/benday-core/src/compile/xy.rs:30-35, 56-73, 129-133,
  203-218, 219-223` (y gate, row parsing, scale, axis, domain output)
- Modify: `crates/benday-core/src/compile/mod.rs` (bar_route temporal
  error; error constructors near `:315`)
- Modify: `crates/benday-core/src/scene.rs:156-168` (Source), `:254-260`
  (meta x type)
- Rename: `tests/cases/declared_date_ordinal.json` →
  `declared_date_temporal.json` (+ its snapshot)
- Create: `tests/cases/temporal_line_gappy.json`,
  `temporal_promoted_string.json`, `temporal_explicit_ordinal.json`,
  `err_temporal_parse.json`, `err_temporal_y.json`,
  `err_temporal_color.json`, `err_bar_temporal_no_timeunit.json`

**Step 1: Grammar.** `FieldType` gains `Temporal` (serde lowercase gives
`"temporal"` for free). `declared_field_type`: `DATE | DATETIME |
TIMESTAMP | TIME` → `FieldType::Temporal`. REWRITE the `ingest.rs:93-95`
doc comment — it currently states the reversed doctrine ("benday has no
temporal scale on the roadmap"); point it at the design doc. Fix the
`:449` test to expect `Temporal`.

**Step 2: Inference.** `infer_type` gains temporal promotion: while
scanning, track `all_numeric` and `all_temporal` (a non-null string
checked with `time::parse_temporal`). All numeric → Quantitative; else
all temporal → Temporal; else Nominal. A column mixing dates and numbers
is Nominal — no partial promotion, per design.

**Step 3: xy continuous classification.** In `compile/xy.rs` define once:

```rust
let x_cont = matches!(xt, FieldType::Quantitative | FieldType::Temporal);
```

and use it at ALL FOUR fork sites (`:56`, `:129`, `:203`, `:219`).
Within the continuous arm, temporal differs in exactly three places:

- **Row parsing (:56):** temporal reads
  `time::parse_temporal(&data::text(xv))`; an unparseable value in a
  resolved-temporal column is a HARD `Error::Data` naming the row number,
  the offending string, and the four accepted shapes — never a silent
  drop (a PROMOTED column parses by construction; this error fires for
  declared/explicit types).
- **Scale (:129):** `time::temporal_axis(xmin, xmax, plot_w)`; feed its
  domain into `Linear { min, max, step: max - min }` so `norm` and the
  scene contract are unchanged.
- **Axis (:203):** map the axis's `(ms, label)` ticks to columns with
  `xscale.norm` (same arithmetic as the nominal arm), anchors →
  `place_x_labels`. Do NOT call `value_axis_x` (its `fmt_tick` is
  numeric).

The ordinal sort at `:101` keeps testing `Ordinal` exactly as is.

**Step 4: Scene + meta.** `Source` gains
`#[serde(skip_serializing_if = "Option::is_none")] pub x_type:
Option<FieldType>` — set `Some(Temporal)` by the temporal path ONLY,
`None` everywhere else, so every existing snapshot stays byte-identical.
`Scene::meta()` (`scene.rs:258`): categories present → `"nominal"`; else
`x_type == Some(Temporal)` → `"temporal"` with the domain formatted as
ISO strings (add `time::format_iso(ms) -> String`); else
`"quantitative"` with the numeric domain, exactly as today.

**Step 5: Teaching errors** (constructors in `compile/mod.rs`, one each —
error strings are API):

- temporal y (xy gate at `xy.rs:30`): name the resolved type and the fix
  — aggregate it, or put time on x.
- temporal color (after `:37`): reject before it becomes one series per
  timestamp; suggest a categorical field.
- `bar` + temporal x (in `bar_route`): "bars need discrete time buckets;
  add `\"timeUnit\": \"day\"` (or week/month/…) — or use `line`/`point`
  for continuous time."
- temporal parse failure: as in step 3.

**Step 6: Corpus.** New cases from the Files list: a gappy temporal line
(irregular dates — the snapshot must show unequal spacing),
a promoted string column, `"type": "ordinal"` escape hatch pinning
today's equal-spaced behavior, and one case per error. Rename
`declared_date_ordinal` → `declared_date_temporal`.

**Step 7: `make validate` + audit.** Authorized diff: ONLY the renamed
`declared_date_temporal` snapshot — audit it line by line (temporal
domain replaces categories; tick labels from the calendar formatter; the
three monthly points at true positions). ZERO other existing corpus
diffs, ZERO gallery diffs, zero meta changes outside the migrated case.
Render `temporal_line_gappy` in a real terminal and LOOK at it.

**Step 8: Commit:** `feat(compile): temporal x — calendar scale, true positions, teaching errors`

---

## Task 4: `timeUnit` — truncation, densify, temporal bars

**Files:**
- Modify: `crates/benday-core/src/spec.rs:80-87` (Channel), new `TimeUnit`
  enum
- Modify: `crates/benday-core/src/time.rs` (truncation + bucket labels)
- Modify: `crates/benday-core/src/compile/mod.rs:355-400` (scan
  transform), `:427-451` (`aggregate_cells` empty-count rule), `preflight`
  (validation)
- Modify: `crates/benday-core/src/compile/bars.rs:55-62` (plain path gap
  tolerance), `:51-53` (bucket sort)
- Modify: `crates/benday-core/src/scene.rs` (Source gains optional
  `time_unit`, skip-serialized; meta reports it)
- Create: `tests/cases/timeunit_hour_count.json` (with a quiet-hours zero
  run), `timeunit_month_sum.json`, `timeunit_gap_mean.json`,
  `err_timeunit_not_temporal.json`, `err_timeunit_on_y.json`,
  `err_timeunit_line.json`

**Step 1: Grammar.**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit { Year, Quarter, Month, Week, Day, Hour, Minute }
```

`Channel` gains `#[serde(default, rename = "timeUnit")] pub time_unit:
Option<TimeUnit>`. Validation in `preflight` / `bar_route`, each a
teaching error: `timeUnit` on y or color → rejected; `timeUnit` on a
non-temporal x → names the resolved type AND which precedence rung
decided it; `timeUnit` with `line`/`point`/`area` → "timeUnit buckets
bars; line/point/area already plot continuous time — drop timeUnit or
use bar". (Scope decision per design YAGNI: timeUnit is bar-only this
cycle.)

**Step 2: Truncation + labels** in `time.rs`, TDD as before:

```rust
/// Canonical bucket KEY: zero-padded ISO prefix — lexical order IS
/// chronological. year "2026" · quarter "2026-Q2" · month "2026-06" ·
/// week "2026-06-08" (Monday) · day "2026-06-14" · hour "2026-06-14 09h"
/// · minute "2026-06-14 09:12".
pub(crate) fn bucket_key(ms: f64, u: TimeUnit) -> String
/// The key's timestamp (bucket start) and successor, for densify.
pub(crate) fn bucket_start(ms: f64, u: TimeUnit) -> f64
pub(crate) fn next_bucket(ms: f64, u: TimeUnit) -> f64
/// DISPLAY label: same context+delta scheme as axis ticks (task 2 table),
/// context at the first bucket and at rollovers.
pub(crate) fn bucket_display(ms: f64, u: TimeUnit, context: bool) -> String
```

**Step 3: The bar transform.** Before `scan_bars`, when x is temporal
with a timeUnit: build a transformed row set with the x value replaced by
`bucket_key(parse_temporal(x)?, unit)` (unparseable → the task-3 parse
error). Scanner and interning run UNCHANGED. Buckets then sort via the
existing `sort_cats` (force it for the timeUnit path — lexical =
chronological by construction of the keys).

**Step 4: Densify + the empty-bucket rules** (design §compile, pinned
after review):

- After sort, walk `bucket_start(first)..=last` with `next_bucket`,
  inserting missing keys with empty cells at their chronological slot.
- `aggregate_cells` gains a `densified: bool` parameter (all existing
  callers pass `false` — grouped-bar gap behavior untouched). When
  `densified && agg == Count`, an empty cell aggregates to `Some(0.0)`
  (count of an empty set is zero); every other aggregate keeps `None`.
- Plain vertical path (`bars.rs:59-62`): REPLACE the
  `expect("a scanned category has values")` with the grouped path's
  `if let Some(v)` convention (`bars.rs:237`) — a `None` cell emits no
  bar glyph but keeps its category slot and tick: a gap at a stable
  position. Non-timeUnit plain bars still never see `None`; state that
  invariant in a comment where the expect used to be.
- Display labels: category labels shown on the axis are
  `bucket_display` (context at first bucket + rollovers), while KEYS
  stay canonical for grouping/sort/meta.

**Step 5: Meta.** `Source.time_unit: Option<TimeUnit>`, skip-serialized;
`--meta` reports it in the x block alongside the bucket categories.

**Step 6: `make validate` + audit.** ZERO existing-snapshot diffs of any
kind (the `aggregate_cells` parameter and `Option` bar values must be
behavior-preserving for every current case — that is the referee).
New corpus cases from the Files list; `timeunit_hour_count` MUST contain
a quiet-hour run in its input and show zero bars there, and
`timeunit_gap_mean` must show gaps. Render the hour-count case in a real
terminal — this is the raw-gcloud-logs story; look at it.

**Step 7: Commit:** `feat(compile): timeUnit bucketing — canonical ISO keys, densified buckets, temporal bars`

---

## Task 5: Gallery, README, --help, doctrine

**Files:**
- Create: gallery cases (in `tests/gallery.rs` + fixtures): daily line
  (week ticks), quarterly line (rollover), gappy temporal line, hourly
  count bars with quiet hours, month-sum bars — ALL with pinned explicit
  sizes
- Modify: `README.md` (spec grammar block: `"temporal"`, `"timeUnit"`;
  a temporal example)
- Modify: `crates/benday-cli/src/main.rs:9` (`EXAMPLES`): the temporal
  section — accepted formats, the UTC-naive convention, and TWO worked
  examples per design: a quarterly trend line, and raw log timestamps →
  hourly bar counts
- Modify: `CLAUDE.md` (contracts section): rewrite the "SQL owns sorting,
  bucketing, and date formatting … No sort grammar, no temporal scales"
  bullet — SQL still owns sorting and owns bucketing when present; benday
  owns time when SQL is absent and positional truth always; link the
  design doc
- Modify: `docs/plans/2026-07-04-bar-family-design.md` — do NOT touch
  (dated record); this line is here to remind the executor of that rule

**Steps:** gallery TDD (write case, review rendered snapshot against the
task-2 label table, accept), then docs, then `make validate` (zero diffs
outside the new gallery snapshots), then a real-terminal render of every
new gallery case at its pinned size.

**Commit:** `docs: temporal family — gallery, README, --help temporal section, doctrine update`

---

## Execution notes for the orchestrator

- Task order is strict; phase 1 (tasks 1–3) fully green before task 4.
- After each task: adversarial subagent review against BOTH this plan and
  the design doc, then `make validate` run BY YOU, then the snapshot
  audit. Never accept a diff you cannot explain line by line.
- The design doc is the semantic authority. Where this plan and the
  design disagree, STOP and reconcile with the human before proceeding.
- Error strings are API: copy them from the design doc exactly; every
  error names the fix.
