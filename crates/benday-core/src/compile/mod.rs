//! The compiler: resolve a spec against its data into a `Scene` — every
//! data- and layout-dependent decision made, geometry normalized to the plot
//! rect. All colors are resolved here; the rasterizer never sees a `Theme`.
//!
//! Every mark compiles here: bars to `SceneMark::Bars`, and line/point/area to
//! per-series `Path`/`Points`/`Fill` marks whose points are normalized to the
//! plot rect. `preflight()` is the shared spec/data validation run before
//! dispatch. Geometry convention: a mark point is `[frac_x, frac_y]` where
//! `frac_x = xscale.norm(x)` (0 at the left edge) and `frac_y` is already
//! FLIPPED to `1 - yscale.norm(y)` (0 at the top edge) so the rasterizer only
//! multiplies by its pixel grid — it never re-flips.

use crate::data;
use crate::error::Error;
use crate::ingest::{Row, Table};
use crate::scale::{fmt_tick, Linear};
use crate::scene::{
    Bar, BarDirection, Chrome, LegendEntry, Placed, Rect, Scene, SceneMark, SeriesRef, Size,
    Source, XAxis, YAxis, YTick,
};
use crate::spec::{Aggregate, Channel, FieldType, Mark, Spec, TimeUnit};
use crate::theme::Theme;
use std::collections::HashMap;

mod bars;
mod xy;

use bars::{compile_bar, compile_bar_h};
use xy::compile_xy;

// 12 row-intervals divide evenly by 2/3/4/6 for the row-aligned tick search;
// width 72 plus the axis gutter stays under 80 columns.
const DEFAULT_WIDTH: usize = 72;
const DEFAULT_HEIGHT: usize = 13;

pub struct CompileOptions {
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub theme: Theme,
}

/// Plot area dimensions: caller override, else spec, else defaults; floored so
/// there is always room for a chart.
pub(crate) fn plot_dims(
    width: Option<usize>,
    height: Option<usize>,
    spec: &Spec,
) -> (usize, usize) {
    let plot_w = width.or(spec.width).unwrap_or(DEFAULT_WIDTH).max(8);
    let plot_h = height.or(spec.height).unwrap_or(DEFAULT_HEIGHT).max(3);
    (plot_w, plot_h)
}

/// Y ticks for a row-aligned scale: k intervals over plot_h-1 rows with
/// exact integer spacing, one YTick per scale tick, rows descending from
/// the bottom. `top` is the plot's buffer-absolute first row.
fn y_ticks(y: &Linear, plot_h: usize, top: usize) -> Vec<YTick> {
    let k = ((y.max - y.min) / y.step).round() as usize;
    let spacing = (plot_h - 1) / k;
    y.ticks()
        .iter()
        .enumerate()
        .map(|(i, &t)| YTick {
            value: t,
            frac: y.norm(t),
            label: fmt_tick(t, y.step),
            row: top + (plot_h - 1) - i * spacing,
        })
        .collect()
}

/// Spec- and data-level rules the type system can't express, run before either
/// render path. Loud by design: a silently ignored channel produces a chart
/// the caller didn't ask for, which an agent reading dot art cannot detect.
pub fn preflight(spec: &Spec, rows: &[Row]) -> Result<(), Error> {
    validate(spec)?;
    if spec.mark == Mark::Bar {
        // Bar field checks are orientation-neutral, and must run BEFORE
        // orientation is resolved: an absent field infers Nominal, so resolving
        // orientation first would misroute a missing value field into the
        // both-categorical error instead of reporting the missing field. Each
        // channel's field must exist unless that channel is an intrinsic
        // `count` (count needs no field). The category channel never carries
        // count — count forces its channel to be the quantitative value axis —
        // so this checks the category unconditionally and the value axis unless
        // it's a count, in EITHER orientation.
        if !matches!(spec.encoding.x.aggregate, Some(Aggregate::Count)) {
            data::check_field(rows, &spec.encoding.x.field)?;
        }
        if !matches!(spec.encoding.y.aggregate, Some(Aggregate::Count)) {
            data::check_field(rows, &spec.encoding.y.field)?;
        }
    } else {
        data::check_field(rows, &spec.encoding.x.field)?;
        if !matches!(spec.encoding.y.aggregate, Some(Aggregate::Count)) {
            data::check_field(rows, &spec.encoding.y.field)?;
        }
    }
    if let Some(c) = &spec.encoding.color {
        data::check_field(rows, &c.field)?;
    }
    Ok(())
}

fn validate(spec: &Spec) -> Result<(), Error> {
    if spec.encoding.x_offset.is_some() {
        return Err(Error::Spec(
            "`xOffset` is not supported; grouping is expressed with color alone \
             — set encoding.color to the grouping field"
                .into(),
        ));
    }
    // Aggregate-on-x is a blanket error for NON-bar marks only. For bars,
    // quantitative x is now a legal (horizontal) route and `aggregate` placement
    // is checked post-orientation, per compiler, against the CATEGORICAL channel.
    if spec.mark != Mark::Bar && spec.encoding.x.aggregate.is_some() {
        return Err(Error::Spec(
            "`aggregate` on encoding.x is not supported; aggregation runs over y, grouped by x"
                .into(),
        ));
    }
    if let Some(c) = &spec.encoding.color {
        if c.aggregate.is_some() {
            return Err(Error::Spec(
                "`aggregate` on encoding.color is not supported; put it on encoding.y".into(),
            ));
        }
    }
    // `timeUnit` is bar-only and x-only this cycle (design §spec semantics).
    // Reject the three misplacements up front, each naming the fix; the
    // non-temporal-x case needs the resolved type and so lives in `bar_route`.
    if spec.encoding.y.time_unit.is_some() {
        return Err(timeunit_channel_error("y"));
    }
    if spec
        .encoding
        .color
        .as_ref()
        .is_some_and(|c| c.time_unit.is_some())
    {
        return Err(timeunit_channel_error("color"));
    }
    if spec.encoding.x.time_unit.is_some() && spec.mark != Mark::Bar {
        return Err(timeunit_mark_error(spec.mark));
    }
    Ok(())
}

/// Which way a bar chart runs. Resolved once, up front, from the count rule and
/// the channel-type precedence chain.
enum BarRoute {
    Vertical,
    Horizontal,
}

/// Resolve bar orientation. RESOLUTION ORDER IS A HARDENED CONTRACT:
///   1. Count-on-both is ill-formed regardless of anything else → error.
///   2. x-count makes x THE quantitative value channel (count is intrinsically
///      numeric and ignores the field's values, so even a temporal field counts
///      fine) → horizontal, BEFORE the temporal gate. `timeUnit` here has no
///      time axis to bucket: teaching error, never silently ignored.
///   3. The temporal/timeUnit gate, BEFORE the y-count rule (a y-count would
///      otherwise short-circuit a temporal x into a raw-timestamp category
///      axis). `timeUnit` present is EXPLICIT temporal intent, so it gates on
///      the CANONICAL chain (`resolved_type`, whose inference rung PROMOTES an
///      all-ISO-string column) — undeclared raw-log timestamps bucket without
///      a declaration, the design's no-SQL workload. Non-temporal under that
///      chain → teaching error naming the type and the deciding rung. WITHOUT
///      a `timeUnit`, the native rung governs as always: declared/explicit
///      temporal → teaching error; undeclared date strings stay categorical
///      (the recorded stdin contract — see the closure below).
///   4. y-count → vertical (count's field may be absent from rows entirely,
///      inferring Nominal — which must not misroute through the type pair).
///   5. Otherwise route both channel types (resolved through precedence: spec
///      `type` > declared column type > inference) by the type pair.
///   6. Both-categorical: a coercion rescue (stdin-cycle contract) reconsiders
///      channels WITHOUT an explicit spec `type`, biasing to vertical.
fn bar_route(spec: &Spec, table: &Table) -> Result<BarRoute, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let x_count = matches!(spec.encoding.x.aggregate, Some(Aggregate::Count));
    let y_count = matches!(spec.encoding.y.aggregate, Some(Aggregate::Count));

    // 1. Count-on-both.
    if x_count && y_count {
        return Err(Error::Spec(
            "aggregate belongs on exactly one channel".into(),
        ));
    }
    // 2. x-count → horizontal, ahead of the temporal gate: count makes x the
    // VALUE axis and never reads the field's values, so a temporal x is no
    // obstacle — but a `timeUnit` on it has no time axis to bucket.
    if x_count {
        if spec.encoding.x.time_unit.is_some() {
            return Err(timeunit_xcount_error());
        }
        return Ok(BarRoute::Horizontal);
    }

    // Resolve both channel types through the precedence chain. The inference
    // rung is NATIVE-typed: a JSON string is categorical-SHAPED even when its
    // contents are numeric (e.g. dice faces "1".."6"), so a string x stays a
    // category and the bar stays vertical. Numeric strings that genuinely belong
    // on the value axis are recovered by the coercion rescue below.
    let resolve = |ch: &Channel, f: &str| -> FieldType {
        ch.ty
            .or_else(|| table.declared.get(f).copied())
            .unwrap_or_else(|| native_type(rows, f))
    };
    let xt = resolve(&spec.encoding.x, xf);

    // 3. The temporal/timeUnit gate, before the y-count rule (a temporal x is
    // the vertical bucket/category axis in every remaining shape).
    if spec.encoding.x.time_unit.is_some() {
        // `timeUnit` is explicit temporal intent, so the gate uses CANONICAL
        // resolution — spec `type` > declared > `data::infer_type`, whose
        // inference rung PROMOTES an undeclared all-ISO-string column — not
        // the native rung above. Raw JSON logs bucket without a declaration.
        // A non-temporal x names the type it resolved to AND which precedence
        // rung decided — a teaching error.
        let xt = resolved_type(&spec.encoding.x, table);
        if xt != FieldType::Temporal {
            return Err(timeunit_not_temporal_error(spec, table, xf, xt));
        }
        // Temporal + timeUnit: the vertical bucket path (compile_bar buckets on
        // the timeUnit). A misplaced non-count aggregate on x is caught there.
        return Ok(BarRoute::Vertical);
    }
    // Native inference never yields Temporal, so `xt == Temporal` here means a
    // declared or explicit temporal x — undeclared date strings stay on the
    // categorical routes below, exactly as before timeUnit existed.
    if xt == FieldType::Temporal {
        // A temporal x with no buckets to draw; name the fix (`timeUnit`).
        return Err(bar_temporal_error());
    }

    // 4. y-count → vertical.
    if y_count {
        return Ok(BarRoute::Vertical);
    }

    // 5. Route by the type pair (x already resolved above).
    let x_quant = xt == FieldType::Quantitative;
    let y_quant = resolve(&spec.encoding.y, yf) == FieldType::Quantitative;
    match (x_quant, y_quant) {
        (false, true) => Ok(BarRoute::Vertical),
        (true, false) => Ok(BarRoute::Horizontal),
        (true, true) => Err(bar_channel_error(xf, yf, "quantitative")),
        (false, false) => {
            // 6. Coercion rescue — ONLY channels without an explicit spec type
            // (an explicit `"type"` is stated intent, never overridden). Bias
            // to vertical (compat) when y coerces numeric, else horizontal.
            if spec.encoding.y.ty.is_none() && data::infer_type(rows, yf) == FieldType::Quantitative
            {
                Ok(BarRoute::Vertical)
            } else if spec.encoding.x.ty.is_none()
                && data::infer_type(rows, xf) == FieldType::Quantitative
            {
                Ok(BarRoute::Horizontal)
            } else {
                Err(bar_channel_error(xf, yf, "categorical"))
            }
        }
    }
}

/// Quantitative iff the field has a value and every present, non-null value is
/// a NATIVE JSON number. Unlike `data::infer_type`, numeric STRINGS do not
/// count: for orientation, a string-shaped field is a category (its values may
/// still be coerced onto the value axis by the rescue or by `data::num`).
fn native_type(rows: &[Row], field: &str) -> FieldType {
    let mut saw_value = false;
    for row in rows {
        if let Some(v) = row.get(field) {
            if v.is_null() {
                continue;
            }
            saw_value = true;
            if !v.is_number() {
                return FieldType::Nominal;
            }
        }
    }
    if saw_value {
        FieldType::Quantitative
    } else {
        FieldType::Nominal
    }
}

/// The bar orientation-resolution error, for both-quantitative (`both` =
/// "quantitative") and failed rescue (`both` = "categorical").
fn bar_channel_error(xf: &str, yf: &str, both: &str) -> Error {
    Error::Spec(format!(
        "bar needs one categorical and one quantitative channel; both x (\"{xf}\") and y \
         (\"{yf}\") resolved {both}; put categories on one axis or set an explicit \"type\""
    ))
}

/// Aggregate placed on the CATEGORICAL channel. `value_axis` is the channel that
/// SHOULD carry the aggregate ("y" for vertical, "x" for horizontal).
fn bar_aggregate_error(value_axis: &str) -> Error {
    Error::Spec(format!(
        "aggregation runs over the quantitative channel, grouped by the categorical one; \
         put `aggregate` on encoding.{value_axis}"
    ))
}

pub(crate) fn aggregate(values: &[f64], agg: Aggregate) -> f64 {
    match agg {
        Aggregate::Sum => values.iter().sum(),
        Aggregate::Mean => values.iter().sum::<f64>() / values.len() as f64,
        Aggregate::Median => {
            let mut v = values.to_vec();
            v.sort_by(f64::total_cmp);
            let mid = v.len() / 2;
            if v.len().is_multiple_of(2) {
                (v[mid - 1] + v[mid]) / 2.0
            } else {
                v[mid]
            }
        }
        Aggregate::Min => values.iter().cloned().fold(f64::INFINITY, f64::min),
        Aggregate::Max => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        Aggregate::Count => values.len() as f64,
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// The type-precedence contract, in ONE place: an explicit spec `type` beats a
/// declared column type beats inference from the data. (Bar ORIENTATION
/// resolution deliberately uses a native-typed inference rung instead — see
/// `bar_route`.)
fn resolved_type(ch: &Channel, table: &Table) -> FieldType {
    ch.ty
        .or_else(|| table.declared.get(&ch.field).copied())
        .unwrap_or_else(|| data::infer_type(&table.rows, &ch.field))
}

/// Width of the y-axis label gutter: the widest formatted tick.
fn tick_gutter(scale: &Linear) -> usize {
    scale
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, scale.step).chars().count())
        .max()
        .unwrap_or(1)
}

/// Centered title over the plot, plus the rows it occupies — the title and a
/// blank row of breathing room beneath it, or zero without one.
fn place_title(spec: &Spec, gutter: usize, plot_w: usize) -> (Option<Placed>, usize) {
    let title = spec.title.as_deref().map(|t| Placed {
        text: t.to_string(),
        col: gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2,
        row: 0,
    });
    let rows = if title.is_some() { 2 } else { 0 };
    (title, rows)
}

/// Right-aligned category names for the horizontal-bar gutter, each truncated
/// to 24 cells (with a visible '…'); the gutter is the widest surviving name.
fn name_gutter(cats: &[String]) -> (Vec<String>, usize) {
    let names: Vec<String> = cats.iter().map(|c| truncate(c, 24)).collect();
    let gutter = names.iter().map(|s| s.chars().count()).max().unwrap_or(1);
    (names, gutter)
}

// --- Error constructors. These strings are CONTRACT: agents pattern-match
// them to self-correct, and corpus snapshots pin them — each exists once.

/// Negative bar values are rejected, in every orientation.
fn negative_bar_error() -> Error {
    Error::Data("negative values are not yet supported for mark \"bar\"; use mark \"line\"".into())
}

/// More series than palette colors: color is the sole channel identifying a
/// series, so cycling the palette would make two series indistinguishable.
fn palette_cap_error(n_series: usize, palette_len: usize, color_field: &str) -> Error {
    Error::Data(format!(
        "{n_series} series exceed the {palette_len} distinguishable series colors; \
         aggregate or filter \"{color_field}\""
    ))
}

/// A bar scan that yielded no usable rows.
fn no_rows_error(value_field: &str, cat_field: &str) -> Error {
    Error::Data(format!(
        "no usable rows: field \"{value_field}\" has no numeric values \
         (or \"{cat_field}\" is always missing)"
    ))
}

/// Horizontal-bar content taller than the height ceiling.
fn height_ceiling_error(n_bars: usize, content: usize) -> Error {
    Error::Data(format!(
        "{n_bars} bars need height {content}; filter or aggregate, or raise --height"
    ))
}

/// A `bar` with a temporal x, which has no discrete buckets to draw. The exact
/// string is pinned by the design: `timeUnit` (task 4) is the fix it names.
fn bar_temporal_error() -> Error {
    Error::Spec(
        "bars need discrete time buckets; add `\"timeUnit\": \"day\"` (or week/month/…) \
         — or use `line`/`point` for continuous time"
            .into(),
    )
}

/// Temporal on the y channel: time belongs on x, so name the resolved type and
/// the two fixes instead of falling through to the generic categorical-y error.
fn temporal_y_error(mark: Mark, field: &str) -> Error {
    Error::Data(format!(
        "mark {mark:?} resolved y field \"{field}\" as temporal, but y must be quantitative; \
         put \"{field}\" on encoding.x (time belongs on x), or aggregate it (e.g. count) for a \
         quantitative y"
    ))
}

/// Temporal on the color channel: rejected before it explodes into one series
/// per timestamp. Suggests a categorical grouping field.
fn temporal_color_error(field: &str) -> Error {
    Error::Data(format!(
        "encoding.color field \"{field}\" resolved as temporal; color would split into one \
         series per timestamp — group by a categorical field instead (or bucket time with a \
         phase-2 `timeUnit`)"
    ))
}

/// An unparseable value in a resolved-temporal x column. Names the row, the
/// offending string, and the four accepted shapes — a promoted column parses
/// by construction, so this fires only for declared/explicit temporal types.
fn temporal_parse_error(row: usize, value: &str, field: &str) -> Error {
    Error::Data(format!(
        "row {row}: could not parse \"{value}\" as temporal in column \"{field}\"; accepted: \
         \"2026-07-05\", \"2026-07-05T14:30:00\" (or a space instead of T, optional .fff), either \
         with a trailing \"Z\" or \"±hh:mm\" offset, or a bare \"14:30:00\""
    ))
}

/// The lowercase spelling of a resolved field type, for error prose.
fn type_name(t: FieldType) -> &'static str {
    match t {
        FieldType::Quantitative => "quantitative",
        FieldType::Nominal => "nominal",
        FieldType::Ordinal => "ordinal",
        FieldType::Temporal => "temporal",
    }
}

/// `timeUnit` on an x that did not resolve temporal: names the resolved type
/// AND which precedence rung decided it (the design's teaching requirement),
/// then the two ways to make x temporal.
fn timeunit_not_temporal_error(spec: &Spec, table: &Table, xf: &str, xt: FieldType) -> Error {
    let rung = if spec.encoding.x.ty.is_some() {
        "the explicit spec `type`"
    } else if table.declared.contains_key(xf) {
        "the declared column type"
    } else {
        "inference from the data"
    };
    Error::Spec(format!(
        "`timeUnit` buckets a temporal x, but \"{xf}\" resolved to {} via {rung}; declare the \
         column DATE/DATETIME/TIMESTAMP/TIME, or set encoding.x.type to \"temporal\"",
        type_name(xt)
    ))
}

/// The lowercase spelling of a `timeUnit`, for error prose.
fn unit_name(u: TimeUnit) -> &'static str {
    match u {
        TimeUnit::Year => "year",
        TimeUnit::Quarter => "quarter",
        TimeUnit::Month => "month",
        TimeUnit::Week => "week",
        TimeUnit::Day => "day",
        TimeUnit::Hour => "hour",
        TimeUnit::Minute => "minute",
    }
}

/// A densified `timeUnit` walk wider than the plot: a fine unit over a long
/// span (a month of minutes is 43,200 buckets) would draw sub-column garbage —
/// the vertical twin of the horizontal height ceiling. Counted before
/// materializing; N == width stays legal (one-column bars). The coarsening
/// suggestion names the next-coarser unit(s) from the actual one; year has
/// none, so only the width fix remains.
fn timeunit_overflow_error(n: usize, plot_w: usize, unit: TimeUnit) -> Error {
    let coarser = match unit {
        TimeUnit::Minute => Some("hour or day"),
        TimeUnit::Hour => Some("day or week"),
        TimeUnit::Day => Some("week or month"),
        TimeUnit::Week => Some("month or quarter"),
        TimeUnit::Month => Some("quarter or year"),
        TimeUnit::Quarter => Some("year"),
        TimeUnit::Year => None,
    };
    let fix = match coarser {
        Some(c) => format!("use a coarser timeUnit ({c}) or a wider size"),
        None => "use a wider size".to_string(),
    };
    Error::Data(format!(
        "timeUnit \"{}\" spans {n} buckets but the plot is {plot_w} columns; {fix}",
        unit_name(unit)
    ))
}

/// `timeUnit` on an x that carries `aggregate: "count"`: count makes x the
/// quantitative VALUE axis (a horizontal count-per-category bar), so there is
/// no time axis to bucket — name both ways out.
fn timeunit_xcount_error() -> Error {
    Error::Spec(
        "`timeUnit` buckets the time (category) axis, but `aggregate: \"count\"` makes \
         encoding.x the count value axis; drop `timeUnit`, or move `count` to encoding.y \
         and put the time field on encoding.x with `timeUnit`"
            .into(),
    )
}

/// `timeUnit` placed on y or color: it is x-only (it buckets the category axis).
fn timeunit_channel_error(channel: &str) -> Error {
    Error::Spec(format!(
        "`timeUnit` is only supported on encoding.x (it buckets time on the category axis); \
         remove it from encoding.{channel}"
    ))
}

/// `timeUnit` on a line/point/area mark: those already plot continuous time, so
/// bucketing is redundant — name the two fixes.
fn timeunit_mark_error(mark: Mark) -> Error {
    Error::Spec(format!(
        "`timeUnit` buckets bars into discrete periods; mark {mark:?} already plots continuous \
         time — drop `timeUnit`, or switch to `bar`"
    ))
}

/// `timeUnit` combined with a grouping color: densify is scoped to plain bars
/// (design §compile), so a grouped temporal bar is rejected.
fn timeunit_grouped_error(color_field: &str) -> Error {
    Error::Spec(format!(
        "`timeUnit` bars do not support a grouping `color` (\"{color_field}\") yet; \
         drop `color`, or drop `timeUnit`"
    ))
}

/// One pass over the rows for ANY bar variant: categories (first-seen) down
/// one dimension, series (first-seen; one unnamed series when `series_field`
/// is None) down the other, each cell the raw values awaiting aggregation.
/// A `count` aggregate yields 1.0 per row without reading the value field.
struct BarScan {
    cats: Vec<String>,
    series: Vec<String>,
    /// `[category][series]` → raw values; an empty Vec is a missing cell.
    cells: Vec<Vec<Vec<f64>>>,
    dropped: usize,
}

fn scan_bars(
    rows: &[Row],
    cat_field: &str,
    value_field: &str,
    series_field: Option<&str>,
    agg: Aggregate,
) -> BarScan {
    let mut cats: Vec<String> = Vec::new();
    let mut series: Vec<String> = match series_field {
        Some(_) => Vec::new(),
        None => vec![String::new()],
    };
    let mut raw: HashMap<(usize, usize), Vec<f64>> = HashMap::new();
    let mut dropped = 0usize;
    for row in rows {
        let Some(cv) = row.get(cat_field) else {
            dropped += 1;
            continue;
        };
        let vn = if agg == Aggregate::Count {
            Some(1.0)
        } else {
            row.get(value_field).and_then(data::num)
        };
        let Some(vn) = vn else {
            dropped += 1;
            continue;
        };
        let ci = index_of_or_push(&mut cats, data::text(cv));
        let si = match series_field {
            Some(sf) => {
                let name = row.get(sf).map(data::text).unwrap_or_else(|| "null".into());
                index_of_or_push(&mut series, name)
            }
            None => 0,
        };
        raw.entry((ci, si)).or_default().push(vn);
    }
    let mut cells = vec![vec![Vec::new(); series.len()]; cats.len()];
    for ((ci, si), v) in raw {
        cells[ci][si] = v;
    }
    BarScan {
        cats,
        series,
        cells,
        dropped,
    }
}

/// First-seen interning: the index of `item`, pushing it if new.
fn index_of_or_push(list: &mut Vec<String>, item: String) -> usize {
    match list.iter().position(|s| *s == item) {
        Some(i) => i,
        None => {
            list.push(item);
            list.len() - 1
        }
    }
}

/// Lexical category sort for ordinal axes (ISO dates sort chronologically),
/// carrying each category's cell row along. Series order stays first-seen.
fn sort_cats(cats: Vec<String>, cells: Vec<Vec<Vec<f64>>>) -> (Vec<String>, Vec<Vec<Vec<f64>>>) {
    let mut pairs: Vec<(String, Vec<Vec<f64>>)> = cats.into_iter().zip(cells).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs.into_iter().unzip()
}

/// Aggregate every cell, rejecting negative results, tracking the maximum for
/// the value scale. An empty cell is normally `None` — a visible gap at a
/// stable position. Only the `densified` temporal path (which INSERTS empty
/// buckets for calendar gaps) treats an empty cell under `count` as a
/// well-defined zero; every other aggregate — and every non-densified caller —
/// keeps it `None` (mean of nothing is undefined).
#[allow(clippy::type_complexity)]
fn aggregate_cells(
    cells: &[Vec<Vec<f64>>],
    agg: Aggregate,
    densified: bool,
) -> Result<(Vec<Vec<Option<f64>>>, f64), Error> {
    let mut vmax = f64::NEG_INFINITY;
    let mut out = Vec::with_capacity(cells.len());
    for row in cells {
        let mut out_row = Vec::with_capacity(row.len());
        for values in row {
            if values.is_empty() {
                if densified && agg == Aggregate::Count {
                    vmax = vmax.max(0.0);
                    out_row.push(Some(0.0));
                } else {
                    out_row.push(None);
                }
                continue;
            }
            let v = aggregate(values, agg);
            if v < 0.0 {
                return Err(negative_bar_error());
            }
            vmax = vmax.max(v);
            out_row.push(Some(v));
        }
        out.push(out_row);
    }
    Ok((out, vmax))
}

/// Resolve a spec into a Scene: every data- and layout-dependent decision made,
/// geometry normalized, colors baked in. Bars and xy marks share `preflight`.
pub fn compile(spec: &Spec, table: &Table, opts: &CompileOptions) -> Result<Scene, Error> {
    preflight(spec, &table.rows)?;
    let (plot_w, plot_h) = plot_dims(opts.width, opts.height, spec);
    match spec.mark {
        Mark::Bar => match bar_route(spec, table)? {
            BarRoute::Vertical => compile_bar(spec, table, opts, plot_w, plot_h),
            // Horizontal bars are content-sized: their height is derived from the
            // category count, not `plot_dims` (which collapses "no height" into
            // the default 13 and so can't tell an explicit 13 from the default).
            // Pass the RAW height Option straight through.
            BarRoute::Horizontal => {
                compile_bar_h(spec, table, opts, plot_w, opts.height.or(spec.height))
            }
        },
        Mark::Line | Mark::Point | Mark::Area => compile_xy(spec, table, opts, plot_w, plot_h),
    }
}

/// Map a scale value to its plot-relative x column: norm over `plot_w - 1`
/// columns, half-up rounding, clamped to the last column. The ONE column
/// arithmetic for every x tick — quantitative, temporal, and nominal axes all
/// route through here, and `time::accept` pre-tests temporal rungs against
/// this exact formula (see its doc); changing it in one place is the point.
fn x_col(xscale: &Linear, v: f64, plot_w: usize) -> usize {
    ((xscale.norm(v) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1)
}

/// A quantitative x value axis: plot-relative tick columns plus greedily-placed
/// tick labels. Extracted from `compile_xy`'s quantitative-x branch so the
/// horizontal-bar value axis and the line/point/area value axis share ONE
/// implementation. Behavior-neutral for xy: identical columns, anchors, and
/// `place_x_labels` call as before.
fn value_axis_x(
    xscale: &Linear,
    plot_w: usize,
    gutter: usize,
    columns: usize,
    label_row: usize,
) -> (Vec<usize>, Vec<Placed>) {
    let tks = xscale.ticks();
    let tick_cols: Vec<usize> = tks.iter().map(|t| x_col(xscale, *t, plot_w)).collect();
    let anchors: Vec<(usize, String)> = tick_cols
        .iter()
        .zip(&tks)
        .map(|(c, t)| (*c, fmt_tick(*t, xscale.step)))
        .collect();
    let labels = place_x_labels(&anchors, gutter, columns, label_row);
    (tick_cols, labels)
}

/// The shared multi-series legend: "── name" entries flow below the x labels
/// starting at `top + plot_h + 2`, wrapping before the right edge. Entries are
/// never clipped; a name wider than the whole row is visibly truncated with
/// '…'. Colors cycle the palette by entry index (`theme.series`). Returns the
/// placed entries plus the number of rows they occupy. Used by both the xy
/// (line/point/area) path and the grouped-bar path — ONE implementation.
fn legend_below(
    names: &[String],
    theme: &Theme,
    gutter: usize,
    columns: usize,
    top: usize,
    plot_h: usize,
) -> (Vec<LegendEntry>, usize) {
    let legend_row0 = top + plot_h + 2;
    let left = gutter + 1;
    let max_name = columns.saturating_sub(left + 3);
    let (mut col, mut row) = (left, legend_row0);
    let mut legend: Vec<LegendEntry> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let name = truncate(name, max_name);
        let w = 3 + name.chars().count(); // "── " + name
        if col > left && col + w > columns {
            col = left;
            row += 1;
        }
        legend.push(LegendEntry {
            name,
            color: theme.series(i),
            col,
            row,
        });
        col += w + 3;
    }
    let legend_rows = legend.last().map_or(0, |e| e.row + 1 - legend_row0);
    (legend, legend_rows)
}

/// Greedy left-to-right x-label placement: each label centered on its anchor
/// column, clamped inside the buffer, skipped if it would collide with the one
/// before it. Mirrors the old `draw_x_axis`; survivors carry buffer-absolute
/// start columns.
///
/// `time::accept` is the temporal-axis mirror of this rule, run at gutter 0 and
/// width `plot_w` to pre-test a candidate tick rung: it is deliberately STRICTER
/// (a whole-rung reject, its right clamp one column tighter), so any rung it
/// accepts survives here at any y-gutter — the shift by `gutter` only loosens
/// spacing, never tightens it. Diverge one from the other and temporal ticks
/// silently drop their labels; keep the arithmetic in lockstep.
fn place_x_labels(
    anchors: &[(usize, String)],
    gutter: usize,
    width: usize,
    row: usize,
) -> Vec<Placed> {
    let mut out = Vec::new();
    let mut next_free = 0usize;
    for (col, label) in anchors {
        let len = label.chars().count();
        if len == 0 || len > width {
            continue;
        }
        let start = (gutter + 1 + col).saturating_sub(len / 2).min(width - len);
        if start < next_free {
            continue;
        }
        out.push(Placed {
            text: label.clone(),
            col: start,
            row,
        });
        next_free = start + len + 1;
    }
    out
}
