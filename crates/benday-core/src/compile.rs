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
use crate::spec::{Aggregate, Channel, FieldType, Mark, Spec};
use crate::theme::Theme;
use std::collections::HashMap;

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
    Ok(())
}

/// Which way a bar chart runs. Resolved once, up front, from the count rule and
/// the channel-type precedence chain.
enum BarRoute {
    Vertical,
    Horizontal,
}

/// Resolve bar orientation. RESOLUTION ORDER IS A HARDENED CONTRACT:
///   1. `count` on a channel makes it THE quantitative value channel (count is
///      intrinsically numeric and its field may be absent from rows entirely,
///      inferring Nominal — which must not misroute). y-count → vertical,
///      x-count → horizontal, both → error.
///   2. Otherwise resolve both channel types through precedence (spec `type` >
///      declared column type > inference) and route by the type pair.
///   3. Both-categorical: a coercion rescue (stdin-cycle contract) reconsiders
///      channels WITHOUT an explicit spec `type`, biasing to vertical.
fn bar_route(spec: &Spec, table: &Table) -> Result<BarRoute, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let x_count = matches!(spec.encoding.x.aggregate, Some(Aggregate::Count));
    let y_count = matches!(spec.encoding.y.aggregate, Some(Aggregate::Count));

    // 1. Count rule FIRST.
    match (x_count, y_count) {
        (true, true) => {
            return Err(Error::Spec(
                "aggregate belongs on exactly one channel".into(),
            ));
        }
        (false, true) => return Ok(BarRoute::Vertical),
        (true, false) => return Ok(BarRoute::Horizontal),
        (false, false) => {}
    }

    // 2. Resolve both channel types through the precedence chain. The inference
    // rung is NATIVE-typed: a JSON string is categorical-SHAPED even when its
    // contents are numeric (e.g. dice faces "1".."6"), so a string x stays a
    // category and the bar stays vertical. Numeric strings that genuinely belong
    // on the value axis are recovered by the coercion rescue below.
    let resolve = |ch: &Channel, f: &str| -> FieldType {
        ch.ty
            .or_else(|| table.declared.get(f).copied())
            .unwrap_or_else(|| native_type(rows, f))
    };
    let x_quant = resolve(&spec.encoding.x, xf) == FieldType::Quantitative;
    let y_quant = resolve(&spec.encoding.y, yf) == FieldType::Quantitative;
    match (x_quant, y_quant) {
        (false, true) => Ok(BarRoute::Vertical),
        (true, false) => Ok(BarRoute::Horizontal),
        (true, true) => Err(bar_channel_error(xf, yf, "quantitative")),
        (false, false) => {
            // 3. Coercion rescue — ONLY channels without an explicit spec type
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

/// Whether a color channel splits the bars into grouped series (it names a
/// THIRD field) rather than tinting the category axis (it names the category
/// field itself). Orientation-neutral: `category_field` is x for vertical bars,
/// y for horizontal — so task 4's horizontal path reuses this predicate.
fn is_grouped(color: Option<&Channel>, category_field: &str) -> bool {
    color.is_some_and(|c| c.field != category_field)
}

fn compile_bar(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let agg = spec.encoding.y.aggregate.unwrap_or(Aggregate::Sum);
    let theme = &opts.theme;

    // Aggregate placement (post-orientation): x is the CATEGORICAL channel for
    // vertical bars, so an aggregate there is misplaced — it runs over the
    // quantitative value channel (y), grouped by the category.
    if spec.encoding.x.aggregate.is_some() {
        return Err(bar_aggregate_error("y"));
    }

    // Grouped bars (color names a third field) take a separate path; the code
    // below handles plain (gradient) and categorical-tint (color == x) bars.
    if is_grouped(spec.encoding.color.as_ref(), xf) {
        return compile_bar_grouped(spec, table, opts, plot_w, plot_h);
    }

    let mut cats: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<f64>> = Vec::new();
    let mut dropped = 0usize;
    for row in rows {
        let Some(xv) = row.get(xf) else {
            dropped += 1;
            continue;
        };
        let yn = if agg == Aggregate::Count {
            Some(1.0)
        } else {
            row.get(yf).and_then(data::num)
        };
        let Some(yn) = yn else {
            dropped += 1;
            continue;
        };
        let cat = data::text(xv);
        match cats.iter().position(|c| *c == cat) {
            Some(i) => groups[i].push(yn),
            None => {
                cats.push(cat);
                groups.push(vec![yn]);
            }
        }
    }
    if cats.is_empty() {
        return Err(Error::Data(format!(
            "no usable rows: field \"{yf}\" has no numeric values (or \"{xf}\" is always missing)"
        )));
    }
    // Bars always treat x as categorical; the resolved x type only decides
    // whether categories sort chronologically (ordinal DATE/TIMESTAMP or an
    // explicit ordinal spec type). `compile_bar` is index-free — cats and
    // groups are parallel — so sort the pairs together before aggregation.
    let xt = spec
        .encoding
        .x
        .ty
        .or_else(|| table.declared.get(xf).copied())
        .unwrap_or_else(|| data::infer_type(rows, xf));
    if xt == FieldType::Ordinal {
        let mut pairs: Vec<(String, Vec<f64>)> = cats.into_iter().zip(groups).collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let (c, g): (Vec<String>, Vec<Vec<f64>>) = pairs.into_iter().unzip();
        cats = c;
        groups = g;
    }
    let values: Vec<f64> = groups.iter().map(|g| aggregate(g, agg)).collect();
    if values.iter().any(|v| *v < 0.0) {
        return Err(Error::Data(
            "negative values are not yet supported for mark \"bar\"; use mark \"line\"".into(),
        ));
    }
    let vmax = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y = Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true);
    // Grouped color was routed away above, so any remaining color channel names
    // the x field: a categorical tint (one color per bar, not per series).
    let categorical = spec.encoding.color.is_some();

    // --- Layout.
    // Title gets a blank row beneath it — breathing room (design doc).
    let title_rows = if spec.title.is_some() { 2 } else { 0 };
    let gutter = y
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, y.step).chars().count())
        .max()
        .unwrap_or(1);
    let columns = gutter + 1 + plot_w;
    let total_rows = title_rows + plot_h + 2;
    let top = title_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    let ticks = y_ticks(&y, plot_h, top);

    // Bars + x-label anchors.
    let n = cats.len();
    let step = plot_w as f64 / n as f64;
    let bar_w = ((step * 0.7).floor() as usize).clamp(1, plot_w);
    let label_max = (step.floor() as usize).saturating_sub(1).max(1);

    let mut bars: Vec<Bar> = Vec::new();
    let mut anchors: Vec<(usize, String)> = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let center = (i as f64 + 0.5) * step;
        let x0 = ((center - bar_w as f64 / 2.0).round().max(0.0) as usize).min(plot_w - bar_w);
        let color = if categorical {
            theme.series(i)
        } else {
            theme.grad(y.norm(*v))
        };
        bars.push(Bar {
            x0: x0 as f64 / plot_w as f64,
            y0: 1.0 - y.norm(*v),
            w: bar_w as f64 / plot_w as f64,
            h: y.norm(*v),
            color,
        });
        anchors.push((
            (center.round() as usize).min(plot_w - 1),
            truncate(&cats[i], label_max),
        ));
    }

    let labels = place_x_labels(&anchors, gutter, columns, top + plot_h + 1);

    Ok(Scene {
        size: Size {
            columns,
            rows: total_rows,
        },
        plot: Rect {
            x: gutter + 1,
            y: top,
            w: plot_w,
            h: plot_h,
        },
        chrome: Chrome {
            axis: theme.axis,
            title: theme.title,
        },
        title,
        legend: Vec::<LegendEntry>::new(),
        y_axis: YAxis {
            domain: [y.min, y.max],
            step: y.step,
            categories: None,
            ticks,
        },
        x_axis: XAxis {
            categories: Some(cats),
            domain: None,
            tick_cols: Vec::new(),
            labels,
        },
        marks: vec![SceneMark::Bars {
            bars,
            direction: BarDirection::Vertical,
        }],
        dropped_rows: dropped,
        source: Source {
            mark: Mark::Bar,
            x_field: xf.clone(),
            y_field: yf.clone(),
            aggregate: Some(agg),
            series_points: groups.iter().map(Vec::len).collect(),
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

/// Grouped vertical bars: `color` names a third field, splitting each category
/// into a group of series bars. Layout indexes by series POSITION (not
/// presence), so a missing (category, series) cell leaves a stable empty slot.
fn compile_bar_grouped(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let cf = &spec
        .encoding
        .color
        .as_ref()
        .expect("grouped bars require a color channel")
        .field;
    let agg = spec.encoding.y.aggregate.unwrap_or(Aggregate::Sum);
    let theme = &opts.theme;

    // Scan into (category, series) cells: categories and series in first-seen
    // order, each cell a vector of raw values to aggregate.
    let mut cats: Vec<String> = Vec::new();
    let mut series_names: Vec<String> = Vec::new();
    let mut raw: HashMap<(usize, usize), Vec<f64>> = HashMap::new();
    let mut dropped = 0usize;
    for row in rows {
        let Some(xv) = row.get(xf) else {
            dropped += 1;
            continue;
        };
        let yn = if agg == Aggregate::Count {
            Some(1.0)
        } else {
            row.get(yf).and_then(data::num)
        };
        let Some(yn) = yn else {
            dropped += 1;
            continue;
        };
        let cat = data::text(xv);
        let sname = row.get(cf).map(data::text).unwrap_or_else(|| "null".into());
        let ci = match cats.iter().position(|c| *c == cat) {
            Some(i) => i,
            None => {
                cats.push(cat);
                cats.len() - 1
            }
        };
        let si = match series_names.iter().position(|s| *s == sname) {
            Some(i) => i,
            None => {
                series_names.push(sname);
                series_names.len() - 1
            }
        };
        raw.entry((ci, si)).or_default().push(yn);
    }
    if cats.is_empty() {
        return Err(Error::Data(format!(
            "no usable rows: field \"{yf}\" has no numeric values (or \"{xf}\" is always missing)"
        )));
    }

    // Ordinal x sorts its categories lexically (declared DATE/TIMESTAMP or an
    // explicit ordinal type). Cells are keyed by category index, so remap the
    // keys alongside the sort; series order stays first-seen.
    let xt = spec
        .encoding
        .x
        .ty
        .or_else(|| table.declared.get(xf).copied())
        .unwrap_or_else(|| data::infer_type(rows, xf));
    if xt == FieldType::Ordinal {
        let mut order: Vec<usize> = (0..cats.len()).collect();
        order.sort_by(|&a, &b| cats[a].cmp(&cats[b]));
        let mut remap = vec![0usize; cats.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old] = new;
        }
        let mut sorted = vec![String::new(); cats.len()];
        for (old, c) in cats.into_iter().enumerate() {
            sorted[remap[old]] = c;
        }
        cats = sorted;
        raw = raw
            .into_iter()
            .map(|((ci, si), v)| ((remap[ci], si), v))
            .collect();
    }

    let n_cats = cats.len();
    let n_series = series_names.len();

    // Palette cap (before layout): color is the only channel identifying a
    // series here, so cycling the palette would make two series
    // indistinguishable — reject loudly. Categorical tint stays exempt (routed
    // to compile_bar). Message matches the chrome cycle's multi-series line cap.
    if n_series > theme.palette.len() {
        return Err(Error::Data(format!(
            "{} series exceed the {} distinguishable series colors; aggregate or filter \"{cf}\"",
            n_series,
            theme.palette.len(),
        )));
    }

    // Fit check (before layout): each slot must hold one column per series plus
    // an inter-group gap, so groups never overlap the neighbor slot.
    let req = n_cats * (n_series + 1);
    if plot_w < req {
        return Err(Error::Data(format!(
            "{n_cats} categories × {n_series} series need width ≥ {req}; \
             raise --width, or filter/aggregate"
        )));
    }

    // Aggregate each cell; a missing cell stays `None` (empty slot).
    let mut cells = vec![vec![None::<f64>; n_series]; n_cats];
    for ((ci, si), v) in &raw {
        cells[*ci][*si] = Some(aggregate(v, agg));
    }
    let mut vmax = f64::NEG_INFINITY;
    for cell in cells.iter().flatten().flatten() {
        if *cell < 0.0 {
            return Err(Error::Data(
                "negative values are not yet supported for mark \"bar\"; use mark \"line\"".into(),
            ));
        }
        vmax = vmax.max(*cell);
    }
    let y = Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true);

    // --- Layout (title, gutter, columns) — identical to plain bars.
    let title_rows = if spec.title.is_some() { 2 } else { 0 };
    let gutter = y
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, y.step).chars().count())
        .max()
        .unwrap_or(1);
    let columns = gutter + 1 + plot_w;
    let top = title_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    let ticks = y_ticks(&y, plot_h, top);

    // Per category slot: a group of `n_series` adjacent bars, centered in the
    // slot. Bar `si` keeps the same in-group offset in every slot, so a missing
    // cell leaves a visible gap at a stable position. The fit check above
    // guarantees `floor(step) - 1 >= n_series`, so the clamp is well-formed.
    let step = plot_w as f64 / n_cats as f64;
    let group_w = ((step * 0.7).round() as usize).clamp(n_series, step.floor() as usize - 1);
    let bar_w = (group_w / n_series).max(1);
    let group_span = bar_w * n_series;
    let label_max = (step.floor() as usize).saturating_sub(1).max(1);

    let mut bars: Vec<Bar> = Vec::new();
    let mut anchors: Vec<(usize, String)> = Vec::new();
    for (ci, cell_row) in cells.iter().enumerate() {
        let center = (ci as f64 + 0.5) * step;
        let group_left =
            ((center - group_span as f64 / 2.0).round().max(0.0) as usize).min(plot_w - group_span);
        for (si, cell) in cell_row.iter().enumerate() {
            if let Some(v) = cell {
                let x0 = group_left + si * bar_w;
                bars.push(Bar {
                    x0: x0 as f64 / plot_w as f64,
                    y0: 1.0 - y.norm(*v),
                    w: bar_w as f64 / plot_w as f64,
                    h: y.norm(*v),
                    color: theme.series(si),
                });
            }
        }
        anchors.push((
            (center.round() as usize).min(plot_w - 1),
            truncate(&cats[ci], label_max),
        ));
    }

    let (legend, legend_rows) = legend_below(&series_names, theme, gutter, columns, top, plot_h);
    let total_rows = top + plot_h + 2 + legend_rows;

    let labels = place_x_labels(&anchors, gutter, columns, top + plot_h + 1);

    // Per-series cell counts (the xy shape's points-per-series analog): how many
    // categories the series appears in.
    let series_points: Vec<usize> = (0..n_series)
        .map(|si| cells.iter().filter(|r| r[si].is_some()).count())
        .collect();

    Ok(Scene {
        size: Size {
            columns,
            rows: total_rows,
        },
        plot: Rect {
            x: gutter + 1,
            y: top,
            w: plot_w,
            h: plot_h,
        },
        chrome: Chrome {
            axis: theme.axis,
            title: theme.title,
        },
        title,
        legend,
        y_axis: YAxis {
            domain: [y.min, y.max],
            step: y.step,
            categories: None,
            ticks,
        },
        x_axis: XAxis {
            categories: Some(cats),
            domain: None,
            tick_cols: Vec::new(),
            labels,
        },
        marks: vec![SceneMark::Bars {
            bars,
            direction: BarDirection::Vertical,
        }],
        dropped_rows: dropped,
        source: Source {
            mark: Mark::Bar,
            x_field: xf.clone(),
            y_field: yf.clone(),
            aggregate: Some(agg),
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
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
    let tick_cols: Vec<usize> = tks
        .iter()
        .map(|t| ((xscale.norm(*t) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1))
        .collect();
    let anchors: Vec<(usize, String)> = tick_cols
        .iter()
        .zip(&tks)
        .map(|(c, t)| (*c, fmt_tick(*t, xscale.step)))
        .collect();
    let labels = place_x_labels(&anchors, gutter, columns, label_row);
    (tick_cols, labels)
}

/// Plain HORIZONTAL bars: quantitative x (the value axis) against categorical y
/// (a ranking down the rows). Mirrors `compile_bar` with the axes swapped, but
/// is content-sized in HEIGHT — one row per bar, one blank between — so the
/// height comes from the RAW `Option<usize>` (see `plot_dims` note), not the
/// default-collapsing dimension resolver.
fn compile_bar_h(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    raw_height: Option<usize>,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field; // value (quantitative)
    let yf = &spec.encoding.y.field; // category
    let agg = spec.encoding.x.aggregate.unwrap_or(Aggregate::Sum);
    let theme = &opts.theme;

    // Aggregate placement (post-orientation): y is the CATEGORICAL channel here.
    if spec.encoding.y.aggregate.is_some() {
        return Err(bar_aggregate_error("x"));
    }
    // A color naming a THIRD field (not the category y) would split the bars
    // into grouped series — that is task 4.
    if is_grouped(spec.encoding.color.as_ref(), yf) {
        return Err(Error::Spec(
            "grouped horizontal bars are not supported yet".into(),
        ));
    }

    // Scan: categories from the Y field (first-seen), values from num(x). Count
    // yields 1.0 per row without reading x (mirrors vertical y-count).
    let mut cats: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<f64>> = Vec::new();
    let mut dropped = 0usize;
    for row in rows {
        let Some(yv) = row.get(yf) else {
            dropped += 1;
            continue;
        };
        let xn = if agg == Aggregate::Count {
            Some(1.0)
        } else {
            row.get(xf).and_then(data::num)
        };
        let Some(xn) = xn else {
            dropped += 1;
            continue;
        };
        let cat = data::text(yv);
        match cats.iter().position(|c| *c == cat) {
            Some(i) => groups[i].push(xn),
            None => {
                cats.push(cat);
                groups.push(vec![xn]);
            }
        }
    }
    if cats.is_empty() {
        return Err(Error::Data(format!(
            "no usable rows: field \"{xf}\" has no numeric values (or \"{yf}\" is always missing)"
        )));
    }
    // Ordinal y (declared DATE/TIMESTAMP or an explicit ordinal spec type) sorts
    // its categories lexically; nominal keeps first-seen (ranking) order. cats
    // and groups are parallel, so sort the pairs together.
    let yt = spec
        .encoding
        .y
        .ty
        .or_else(|| table.declared.get(yf).copied())
        .unwrap_or_else(|| data::infer_type(rows, yf));
    if yt == FieldType::Ordinal {
        let mut pairs: Vec<(String, Vec<f64>)> = cats.into_iter().zip(groups).collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let (c, g): (Vec<String>, Vec<Vec<f64>>) = pairs.into_iter().unzip();
        cats = c;
        groups = g;
    }
    let values: Vec<f64> = groups.iter().map(|g| aggregate(g, agg)).collect();
    if values.iter().any(|v| *v < 0.0) {
        return Err(Error::Data(
            "negative values are not yet supported for mark \"bar\"; use mark \"line\"".into(),
        ));
    }
    let vmax = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let xscale = Linear::nice_from(0.0, vmax, (plot_w / 10).clamp(2, 7), true);

    let n = cats.len();

    // Content-sized height: one bar row per category, one blank row between →
    // n*2 - 1. An explicit height is a CEILING (not a target); with none, a
    // safety cap of 40 rows guards against a runaway ranking.
    let content = n * 2 - 1;
    let ceiling = raw_height.unwrap_or(40);
    if content > ceiling {
        return Err(Error::Data(format!(
            "{n} bars need height {content}; filter or aggregate, or raise --height"
        )));
    }
    let plot_h = content;

    // Grouped color was routed away above, so any remaining color channel names
    // the category (y) field: a categorical tint (one color per bar).
    let categorical = spec.encoding.color.is_some();

    // Name gutter: right-aligned category names, each truncated to 24 (with a
    // visible '…'); the gutter is the widest surviving name.
    let names: Vec<String> = cats.iter().map(|c| truncate(c, 24)).collect();
    let gutter = names.iter().map(|s| s.chars().count()).max().unwrap_or(1);
    let cat_scale = Linear::indices(n);

    let title_rows = if spec.title.is_some() { 2 } else { 0 };
    let columns = gutter + 1 + plot_w;
    let total_rows = title_rows + plot_h + 2;
    let top = title_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    // One bar per row, blank row between: category i sits on plot row i*2.
    let rows_f = plot_h as f64;
    let mut bars: Vec<Bar> = Vec::new();
    let mut ticks: Vec<YTick> = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let plot_row = i * 2;
        let color = if categorical {
            theme.series(i)
        } else {
            theme.grad(xscale.norm(*v))
        };
        bars.push(Bar {
            x0: 0.0,
            y0: plot_row as f64 / rows_f,
            w: xscale.norm(*v),
            h: 1.0 / rows_f,
            color,
        });
        ticks.push(YTick {
            value: i as f64,
            frac: cat_scale.norm(i as f64),
            label: names[i].clone(),
            row: top + plot_row,
        });
    }

    let (tick_cols, labels) = value_axis_x(&xscale, plot_w, gutter, columns, top + plot_h + 1);

    Ok(Scene {
        size: Size {
            columns,
            rows: total_rows,
        },
        plot: Rect {
            x: gutter + 1,
            y: top,
            w: plot_w,
            h: plot_h,
        },
        chrome: Chrome {
            axis: theme.axis,
            title: theme.title,
        },
        title,
        legend: Vec::<LegendEntry>::new(),
        // The category axis lives on y; its "ticks" are the bar rows, and the
        // RAW (untruncated) names ride along for the --meta surface.
        y_axis: YAxis {
            domain: [cat_scale.min, cat_scale.max],
            step: cat_scale.step,
            categories: Some(cats),
            ticks,
        },
        // The value axis lives on x: quantitative, with a numeric domain.
        x_axis: XAxis {
            categories: None,
            domain: Some([xscale.min, xscale.max]),
            tick_cols,
            labels,
        },
        marks: vec![SceneMark::Bars {
            bars,
            direction: BarDirection::Horizontal,
        }],
        dropped_rows: dropped,
        source: Source {
            mark: Mark::Bar,
            x_field: xf.clone(),
            y_field: yf.clone(),
            aggregate: Some(agg),
            series_points: groups.iter().map(Vec::len).collect(),
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

/// One resolved line/point/area series: display name plus raw (x, y) points.
struct XySeries {
    name: String,
    points: Vec<(f64, f64)>,
}

/// Compile line/point/area marks: split into series, sort, build scales, and
/// normalize each point to `[frac_x, 1 - frac_y]` (frac_y flipped here — see
/// the module docs). One `SceneMark` per series, in first-seen order.
fn compile_xy(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let theme = &opts.theme;
    let mark = spec.mark;

    // Type resolution precedence: explicit spec type > declared column type >
    // inference from the data.
    let xt = spec
        .encoding
        .x
        .ty
        .or_else(|| table.declared.get(xf).copied())
        .unwrap_or_else(|| data::infer_type(rows, xf));
    let yt = spec
        .encoding
        .y
        .ty
        .or_else(|| table.declared.get(yf).copied())
        .unwrap_or_else(|| data::infer_type(rows, yf));
    if yt != FieldType::Quantitative {
        return Err(Error::Data(format!(
            "mark {mark:?} needs a quantitative y, but field \"{yf}\" holds categorical values; \
             put categories on x, or set encoding.y.type to \"quantitative\" if they are numbers"
        )));
    }

    let series_field = spec
        .encoding
        .color
        .as_ref()
        .map(|c| c.field.clone())
        .filter(|f| f != xf);

    let mut series: Vec<XySeries> = Vec::new();
    let mut x_cats: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for row in rows {
        let (Some(xv), Some(yv)) = (row.get(xf), row.get(yf)) else {
            dropped += 1;
            continue;
        };
        let Some(yn) = data::num(yv) else {
            dropped += 1;
            continue;
        };
        let xn = if xt == FieldType::Quantitative {
            match data::num(xv) {
                Some(v) => v,
                None => {
                    dropped += 1;
                    continue;
                }
            }
        } else {
            let cat = data::text(xv);
            match x_cats.iter().position(|c| *c == cat) {
                Some(i) => i as f64,
                None => {
                    x_cats.push(cat);
                    (x_cats.len() - 1) as f64
                }
            }
        };
        let name = series_field
            .as_ref()
            .map(|f| row.get(f).map(data::text).unwrap_or_else(|| "null".into()))
            .unwrap_or_default();
        let idx = match series.iter().position(|s| s.name == name) {
            Some(i) => i,
            None => {
                series.push(XySeries {
                    name,
                    points: Vec::new(),
                });
                series.len() - 1
            }
        };
        series[idx].points.push((xn, yn));
    }
    if series.iter().all(|s| s.points.is_empty()) {
        return Err(Error::Data(format!(
            "no usable rows: could not read numeric values from \"{yf}\""
        )));
    }
    // Ordinal x (declared DATE/TIMESTAMP or explicit spec type) sorts its
    // category list lexically so ISO dates plot chronologically even when rows
    // arrive shuffled. The category indices were assigned in first-seen order
    // DURING the scan and stored in every point, so sorting `x_cats` alone
    // would desync labels from points — remap the points too, before the
    // per-series sort re-orders them by x.
    if xt == FieldType::Ordinal {
        let mut sorted = x_cats.clone();
        sorted.sort_unstable();
        // old index -> new index
        let remap: Vec<usize> = x_cats
            .iter()
            .map(|c| sorted.iter().position(|s| s == c).expect("same elements"))
            .collect();
        for s in &mut series {
            for p in &mut s.points {
                p.0 = remap[p.0 as usize] as f64;
            }
        }
        x_cats = sorted;
    }
    for s in &mut series {
        s.points.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    let (mut xmin, mut xmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
    for (x, y) in series.iter().flat_map(|s| s.points.iter()) {
        xmin = xmin.min(*x);
        xmax = xmax.max(*x);
        ymin = ymin.min(*y);
        ymax = ymax.max(*y);
    }
    let yscale = Linear::row_aligned(ymin, ymax, plot_h.clamp(3, 6), plot_h, mark == Mark::Area);
    let xscale = if xt == FieldType::Quantitative {
        Linear::nice_from(xmin, xmax, (plot_w / 10).clamp(2, 7), false)
    } else {
        Linear::indices(x_cats.len())
    };

    let multi = series.len() > 1 || series_field.is_some();

    // Color is the ONLY channel identifying an xy series, so cycling the
    // palette would make two series indistinguishable — reject loudly.
    // (Categorical bars may cycle: each bar is identified by its x label.)
    if series.len() > theme.palette.len() {
        let cf = series_field
            .as_deref()
            .expect("more than one series requires a color field");
        return Err(Error::Data(format!(
            "{} series exceed the {} distinguishable series colors; aggregate or filter \"{cf}\"",
            series.len(),
            theme.palette.len(),
        )));
    }

    // --- Layout: optional title row above the plot, y gutter to its left,
    // legend below the x labels.
    // Title gets a blank row beneath it — breathing room (design doc).
    let title_rows = if spec.title.is_some() { 2 } else { 0 };
    let gutter = yscale
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, yscale.step).chars().count())
        .max()
        .unwrap_or(1);
    let columns = gutter + 1 + plot_w;
    let top = title_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    // Legend (multi-series only): "── name" entries flow below the x labels,
    // wrapping before the right edge. Entries are never clipped; a name wider
    // than the whole row is visibly truncated with '…'.
    let (legend, legend_rows) = if multi {
        let names: Vec<String> = series.iter().map(|s| s.name.clone()).collect();
        legend_below(&names, theme, gutter, columns, top, plot_h)
    } else {
        (Vec::new(), 0)
    };
    let total_rows = top + plot_h + 2 + legend_rows;

    let ticks = y_ticks(&yscale, plot_h, top);

    // One SceneMark per series; points normalized with frac_y flipped here.
    let mut marks: Vec<SceneMark> = Vec::new();
    for (si, s) in series.iter().enumerate() {
        let color = if multi {
            theme.series(si)
        } else {
            theme.accent
        };
        let name = if multi { Some(s.name.clone()) } else { None };
        let sref = SeriesRef { name, color };
        let points: Vec<[f64; 2]> = s
            .points
            .iter()
            .map(|(x, y)| [xscale.norm(*x), 1.0 - yscale.norm(*y)])
            .collect();
        marks.push(match mark {
            Mark::Line => SceneMark::Path {
                series: sref,
                points,
            },
            Mark::Point => SceneMark::Points {
                series: sref,
                points,
            },
            Mark::Area => SceneMark::Fill {
                series: sref,
                points,
            },
            Mark::Bar => unreachable!("bar handled by compile_bar"),
        });
    }

    // X axis: tick columns (plot-relative) + placed labels. Quantitative x
    // shares the value-axis block with horizontal bars (ONE implementation);
    // nominal x lays ticks on the category indices.
    let label_row = top + plot_h + 1;
    let (tick_cols, labels): (Vec<usize>, Vec<Placed>) = if xt == FieldType::Quantitative {
        value_axis_x(&xscale, plot_w, gutter, columns, label_row)
    } else {
        let cols: Vec<usize> = (0..x_cats.len())
            .map(|i| {
                ((xscale.norm(i as f64) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1)
            })
            .collect();
        let anchors: Vec<(usize, String)> = cols
            .iter()
            .zip(&x_cats)
            .map(|(c, name)| (*c, truncate(name, 12)))
            .collect();
        let labels = place_x_labels(&anchors, gutter, columns, label_row);
        (cols, labels)
    };

    let (categories, domain) = if xt == FieldType::Quantitative {
        (None, Some([xscale.min, xscale.max]))
    } else {
        (Some(x_cats), None)
    };

    Ok(Scene {
        size: Size {
            columns,
            rows: total_rows,
        },
        plot: Rect {
            x: gutter + 1,
            y: top,
            w: plot_w,
            h: plot_h,
        },
        chrome: Chrome {
            axis: theme.axis,
            title: theme.title,
        },
        title,
        legend,
        y_axis: YAxis {
            domain: [yscale.min, yscale.max],
            step: yscale.step,
            categories: None,
            ticks,
        },
        x_axis: XAxis {
            categories,
            domain,
            tick_cols,
            labels,
        },
        marks,
        dropped_rows: dropped,
        source: Source {
            mark,
            x_field: xf.clone(),
            y_field: yf.clone(),
            aggregate: None,
            series_points: series.iter().map(|s| s.points.len()).collect(),
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
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
