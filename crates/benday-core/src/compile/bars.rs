//! The four bar compilers: plain and grouped, vertical and horizontal.
//! Orientation swaps which channel is the category vs. the value axis;
//! grouping adds a series dimension from the color field. All four share the
//! parent module's scan/sort/aggregate stages — each function here is the
//! orchestration plus only the layout specific to its shape.

use super::*;
use crate::spec::TimeUnit;
use crate::time;
use serde_json::Value;

/// Rewrite each row's temporal x into its canonical `timeUnit` bucket KEY
/// (`time::bucket_key`), so the scanner interns identical buckets and
/// `sort_cats`' lexical order is chronological. Returns the transformed rows
/// plus the min/max parsed instants that bound densify. A non-null value that
/// won't parse is the task-3 parse error; a null (or absent) x drops like the
/// xy temporal path — the row loses its category and the scan counts it dropped.
fn bucket_rows(rows: &[Row], xf: &str, unit: TimeUnit) -> Result<(Vec<Row>, f64, f64), Error> {
    let mut out = Vec::with_capacity(rows.len());
    let (mut min_ms, mut max_ms) = (f64::INFINITY, f64::NEG_INFINITY);
    for (ri, row) in rows.iter().enumerate() {
        let mut r = row.clone();
        match row.get(xf) {
            Some(v) if !v.is_null() => {
                let text = data::text(v);
                match time::parse_temporal(&text) {
                    Some(ms) => {
                        min_ms = min_ms.min(ms);
                        max_ms = max_ms.max(ms);
                        r.insert(xf.to_string(), Value::String(time::bucket_key(ms, unit)));
                    }
                    None => return Err(temporal_parse_error(ri, &text, xf)),
                }
            }
            _ => {
                r.remove(xf);
            }
        }
        out.push(r);
    }
    Ok((out, min_ms, max_ms))
}

/// Densify a scanned bucket set: walk every calendar bucket from the first
/// DATA bucket to the last (in ms space, via `next_bucket`), pulling each
/// scanned cell by key and inserting an EMPTY cell where a bucket is missing —
/// a gap at its true chronological slot. Returns the dense keys, cells, and the
/// bucket-start ms per slot (for the display labels). The keys are chronological
/// (= lexical) by construction, so this replaces `sort_cats` on the timeUnit path.
fn densify(
    cats: Vec<String>,
    cells: Vec<Vec<Vec<f64>>>,
    min_ms: f64,
    max_ms: f64,
    unit: TimeUnit,
) -> (Vec<String>, Vec<Vec<Vec<f64>>>, Vec<f64>) {
    let mut by_key: HashMap<String, Vec<Vec<f64>>> = cats.into_iter().zip(cells).collect();
    let (mut dense_cats, mut dense_cells, mut dense_ms) = (Vec::new(), Vec::new(), Vec::new());
    let mut ms = time::bucket_start(min_ms, unit);
    let last = time::bucket_start(max_ms, unit);
    loop {
        let key = time::bucket_key(ms, unit);
        // A plain bar has one (unnamed) series; a missing bucket is one empty
        // series cell — the shape `aggregate_cells` reads as a gap/zero.
        let cell = by_key.remove(&key).unwrap_or_else(|| vec![Vec::new()]);
        dense_cats.push(key);
        dense_cells.push(cell);
        dense_ms.push(ms);
        if ms >= last {
            break;
        }
        ms = time::next_bucket(ms, unit);
    }
    debug_assert!(
        by_key.is_empty(),
        "every data bucket lies within [bucket_start(min), bucket_start(max)]"
    );
    (dense_cats, dense_cells, dense_ms)
}

/// Whether a color channel splits the bars into grouped series (it names a
/// THIRD field) rather than tinting the category axis (it names the category
/// field itself). Orientation-neutral: `category_field` is x for vertical bars,
/// y for horizontal — so task 4's horizontal path reuses this predicate.
fn is_grouped(color: Option<&Channel>, category_field: &str) -> bool {
    color.is_some_and(|c| c.field != category_field)
}

pub(super) fn compile_bar(
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
    // A `timeUnit` rides the PLAIN path only — densify is not defined per series
    // this cycle, so a grouping color with a timeUnit is a teaching error.
    let time_unit = spec.encoding.x.time_unit;
    if is_grouped(spec.encoding.color.as_ref(), xf) {
        if let Some(unit_color) = spec.encoding.color.as_ref().filter(|_| time_unit.is_some()) {
            return Err(timeunit_grouped_error(&unit_color.field));
        }
        return compile_bar_grouped(spec, table, opts, plot_w, plot_h);
    }

    // A `timeUnit` transforms x into canonical bucket keys before the scan
    // (bar_route guarantees x is temporal here); the scanner and interning then
    // run UNCHANGED over the keyed rows, and min/max bound the densify walk.
    let (scan, dense_range) = match time_unit {
        Some(unit) => {
            let (trows, min_ms, max_ms) = bucket_rows(rows, xf, unit)?;
            (scan_bars(&trows, xf, yf, None, agg), Some((min_ms, max_ms)))
        }
        None => (scan_bars(rows, xf, yf, None, agg), None),
    };
    let (mut cats, mut cells, dropped) = (scan.cats, scan.cells, scan.dropped);
    if cats.is_empty() {
        return Err(no_rows_error(yf, xf));
    }
    // On the timeUnit path, DENSIFY: insert an empty cell for every calendar
    // bucket missing between the first and last data bucket (chronological =
    // lexical by key construction, so this also supplies the sort). Otherwise
    // only an explicit ordinal spec type sorts (lexically); nominal — and a
    // PROMOTED temporal x, which the native routing rung sent here as a
    // category axis (first-seen order pinned by bar_promoted_string_nominal) —
    // keeps first-seen order. Declared/explicit temporal never reaches this
    // arm: bar_route rejects it without a timeUnit.
    let mut dense_ms: Vec<f64> = Vec::new();
    if let (Some(unit), Some((min_ms, max_ms))) = (time_unit, dense_range) {
        // Guard BEFORE materializing: a fine unit over a long span (a month of
        // minutes is 43,200 buckets) would silently draw sub-column garbage.
        // Counted arithmetically, so the pathological case costs nothing.
        // N == plot_w stays legal: one-column bars.
        let n = time::bucket_count(min_ms, max_ms, unit);
        if n > plot_w {
            return Err(timeunit_overflow_error(n, plot_w, unit));
        }
        (cats, cells, dense_ms) = densify(cats, cells, min_ms, max_ms, unit);
    } else if resolved_type(&spec.encoding.x, table) == FieldType::Ordinal {
        (cats, cells) = sort_cats(cats, cells);
    }
    // Raw per-category value counts (post-densify, so a gap reads 0), for --meta.
    let series_points: Vec<usize> = cells.iter().map(|r| r[0].len()).collect();
    let (agg_cells, vmax) = aggregate_cells(&cells, agg, time_unit.is_some())?;
    let y = Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true);
    // Grouped color was routed away above, so any remaining color channel names
    // the x field: a categorical tint (one color per bar, not per series).
    let categorical = spec.encoding.color.is_some();

    // --- Layout.
    let gutter = tick_gutter(&y);
    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let total_rows = title_rows + plot_h + 2;
    let top = title_rows;

    let ticks = y_ticks(&y, plot_h, top);

    // Bars + x-label anchors.
    let n = cats.len();
    let step = plot_w as f64 / n as f64;
    let bar_w = ((step * 0.7).floor() as usize).clamp(1, plot_w);
    let label_max = (step.floor() as usize).saturating_sub(1).max(1);

    let mut bars: Vec<Bar> = Vec::new();
    let mut anchors: Vec<(usize, String)> = Vec::new();
    // Context labels (the task-2 idiom) roll over as the buckets are walked.
    let mut prev_ms: Option<f64> = None;
    for (i, cell) in agg_cells.iter().enumerate() {
        let center = (i as f64 + 0.5) * step;
        // A plain NON-timeUnit bar never sees None: a category exists only
        // because a row landed in it. The densified timeUnit path does — a None
        // cell (a gap under a non-count aggregate) emits no bar glyph but keeps
        // its category slot and tick, exactly as the grouped path treats a
        // missing (category, series) cell.
        if let Some(v) = cell[0] {
            let x0 = ((center - bar_w as f64 / 2.0).round().max(0.0) as usize).min(plot_w - bar_w);
            let color = if categorical {
                theme.series(i)
            } else {
                theme.grad(y.norm(v))
            };
            bars.push(Bar {
                x0: x0 as f64 / plot_w as f64,
                y0: 1.0 - y.norm(v),
                w: bar_w as f64 / plot_w as f64,
                h: y.norm(v),
                color,
            });
        }
        // KEYS stay canonical (grouping/sort/meta); the axis LABEL is the
        // bucket_display context+delta form on the timeUnit path — passed
        // WHOLE, never slot-truncated: the tick idiom includes PLACEMENT, and
        // `place_x_labels` centers each label on its slot, spans free neighbor
        // columns, and drops on collision. Its greedy pass runs left-to-right,
        // so the first (context) label survives preferentially and later
        // deltas drop around it. Nominal bars keep per-slot truncation.
        let label = match time_unit {
            Some(unit) => {
                let ms = dense_ms[i];
                let label = time::bucket_display(ms, unit, prev_ms);
                prev_ms = Some(ms);
                label
            }
            None => truncate(&cats[i], label_max),
        };
        anchors.push(((center.round() as usize).min(plot_w - 1), label));
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
            x_type: None,
            time_unit,
            bin: None,
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

/// A HISTOGRAM: an active `bin` on a quantitative x, drawn as contiguous
/// vertical bars over a linear, edge-ticked value axis. Selection reuses the
/// bin primitives; contiguity is computed in INTEGER CELL edges so the
/// rasterizer's own independent rounding recovers a gapless tiling (design
/// §Geometry) with zero rasterizer changes. Bins are DENSE over [lo, hi] —
/// every bin exists; an empty one is a zero bar under `count`, a stable gap
/// under any other aggregate (the timeUnit densify rule, reused not forked).
pub(super) fn compile_histogram(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    // bar_route proved y carries an aggregate; that IS the histogram aggregate.
    let agg = spec
        .encoding
        .y
        .aggregate
        .expect("histogram route implies an aggregated y");
    let theme = &opts.theme;

    // Aggregate placement: x is the BINNED channel, so an aggregate there is
    // misplaced — aggregation runs over y, grouped by the bins. Mirrors the
    // vertical-bar guard; the field is reported, never silently ignored.
    if spec.encoding.x.aggregate.is_some() {
        return Err(bar_aggregate_error("y"));
    }

    // Scan: x through `data::num` — a non-numeric value DROPS. This is legal
    // ONLY because routing resolved x quantitative by EXPLICIT spec type or
    // DECLARED column type; an INFERRED quantitative is all-numeric by the
    // all-or-nothing `infer_type` rule, so nothing drops on that path (invariant
    // asserted by construction, not at runtime). y is the existing bar value
    // scan: count → 1.0, else num(y). A dropped row extends no domain — min/max
    // track usable rows only.
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    let mut dropped = 0usize;
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for row in rows {
        let Some(xn) = row.get(xf).and_then(data::num) else {
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
        min = min.min(xn);
        max = max.max(xn);
        pairs.push((xn, yn));
    }
    if pairs.is_empty() {
        return Err(no_rows_error(yf, xf));
    }

    // Select bins per the knob. Degenerate spans (all values equal, one row)
    // are guarded inside each selector (min..min+1), so a single distinct value
    // bins without dividing by zero — every value lands in one bin. `target`
    // is the design's plot-relative default.
    let bin = active_bin(&spec.encoding.x).expect("histogram route implies an active x bin");
    let target = (plot_w / 4).clamp(5, 20);
    let bins = match bin {
        BinValue::Config(BinConfig {
            maxbins: Some(m), ..
        }) => bins_maxbins(min, max, m as usize),
        BinValue::Config(BinConfig { step: Some(s), .. }) => bins_step(min, max, s),
        // `true` / `{}` (and the unreachable `false`) → automatic binning.
        _ => bins_auto(min, max, target),
    };
    // Checked AFTER selection: a bin count over the plot width would draw
    // sub-column garbage, and this also backstops zero-cell-wide bins. Names
    // whichever knob drove the count.
    if bins.n > plot_w {
        return Err(bin_overflow_error(bins.n, plot_w, bin));
    }

    // Dense cells: one (unnamed) series per bin; push each row's y into its
    // bin. Every bin exists by construction, so `aggregate_cells` with the
    // densified flag reads an empty bin as a zero under `count` and a gap under
    // any other aggregate — the SAME rule the timeUnit densify path uses.
    let mut cells: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new()]; bins.n];
    for &(xn, yn) in &pairs {
        cells[bins.index(xn)][0].push(yn);
    }
    let series_points: Vec<usize> = cells.iter().map(|r| r[0].len()).collect();
    let (agg_cells, vmax) = aggregate_cells(&cells, agg, true)?;
    let y = Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true);

    // --- Layout (title, gutter, columns) — identical to plain bars.
    let gutter = tick_gutter(&y);
    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let total_rows = title_rows + plot_h + 2;
    let top = title_rows;

    let ticks = y_ticks(&y, plot_h, top);

    // Rects tile the plot in INTEGER cells: round the shared edges first, store
    // each bar's x0/w as those integers over plot_w; the rasterizer's own
    // rounding recovers them exactly, so bars touch with no gap or overlap. A
    // `None` cell (a gap under a non-count aggregate) emits no rect but keeps
    // its edge tick — a stable hole in the silhouette.
    let edges = cell_edges(bins.n, plot_w);
    let mut bars: Vec<Bar> = Vec::new();
    for (i, cell) in agg_cells.iter().enumerate() {
        if let Some(v) = cell[0] {
            bars.push(Bar {
                x0: edges[i] as f64 / plot_w as f64,
                y0: 1.0 - y.norm(v),
                w: (edges[i + 1] - edges[i]) as f64 / plot_w as f64,
                h: y.norm(v),
                color: theme.grad(y.norm(v)),
            });
        }
    }

    // Axis: a linear value axis over [lo, hi]. Tick GLYPHS sit at the left cell
    // edge of every bin (`edges[0..n]`, all < plot_w); the right domain edge
    // (column plot_w) is unrepresentable as a glyph (design §Axis). LABELS thin
    // greedily while the ticks stay: each interior edge value anchored at its
    // edge column, then the right domain edge (hi) as a right-aligned label at
    // the buffer end, dropped only if it would collide with the last survivor.
    let lo = bins.lo;
    let hi = bins.hi();
    let tick_cols: Vec<usize> = edges[..bins.n].to_vec();
    let anchors: Vec<(usize, String)> = (0..bins.n)
        .map(|k| (edges[k], fmt_tick(lo + k as f64 * bins.step, bins.step)))
        .collect();
    let label_row = top + plot_h + 1;
    let mut labels = place_x_labels(&anchors, gutter, columns, label_row);
    let hi_label = fmt_tick(hi, bins.step);
    let hi_len = hi_label.chars().count();
    if hi_len <= columns {
        let start = columns - hi_len;
        let next_free = labels
            .last()
            .map_or(0, |l| l.col + l.text.chars().count() + 1);
        if start >= next_free {
            labels.push(Placed {
                text: hi_label,
                col: start,
                row: label_row,
            });
        }
    }

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
            categories: None,
            domain: Some([lo, hi]),
            tick_cols,
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
            x_type: None,
            time_unit: None,
            bin: Some(BinInfo {
                step: bins.step,
                domain: [lo, hi],
                bins: bins.n,
            }),
            series_points,
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

    let scan = scan_bars(rows, xf, yf, Some(cf), agg);
    let (mut cats, mut raw_cells, dropped) = (scan.cats, scan.cells, scan.dropped);
    let series_names = scan.series;
    if cats.is_empty() {
        return Err(no_rows_error(yf, xf));
    }

    // An explicit ordinal x sorts its categories lexically, and so does a
    // temporal one — lexical ISO order IS chronological. (Here temporal means
    // PROMOTED strings only: declared/explicit temporal x is rejected upstream
    // by bar_route without buckets, and timeUnit+color is rejected before this
    // path.) Series order stays first-seen.
    if matches!(
        resolved_type(&spec.encoding.x, table),
        FieldType::Ordinal | FieldType::Temporal
    ) {
        (cats, raw_cells) = sort_cats(cats, raw_cells);
    }

    let n_cats = cats.len();
    let n_series = series_names.len();

    // Palette cap (before layout): color is the only channel identifying a
    // series here — categorical tint stays exempt (routed to compile_bar).
    if n_series > theme.palette.len() {
        return Err(palette_cap_error(n_series, theme.palette.len(), cf));
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

    let (cells, vmax) = aggregate_cells(&raw_cells, agg, false)?;
    let y = Linear::row_aligned(0.0, vmax, plot_h.clamp(3, 6), plot_h, true);

    // --- Layout (title, gutter, columns) — identical to plain bars.
    let gutter = tick_gutter(&y);
    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let top = title_rows;

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
            x_type: None,
            time_unit: None,
            bin: None,
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

/// Plain HORIZONTAL bars: quantitative x (the value axis) against categorical y
/// (a ranking down the rows). Mirrors `compile_bar` with the axes swapped, but
/// is content-sized in HEIGHT — one row per bar, one blank between — so the
/// height comes from the RAW `Option<usize>` (see `plot_dims` note), not the
/// default-collapsing dimension resolver.
pub(super) fn compile_bar_h(
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
    // A color naming a THIRD field (not the category y) splits the bars into
    // grouped series — the horizontal analog of `compile_bar_grouped`, growing
    // the row (series) dimension per category block.
    if is_grouped(spec.encoding.color.as_ref(), yf) {
        return compile_bar_h_grouped(spec, table, opts, plot_w, raw_height);
    }

    // Scan: categories from the Y field, values from num(x) — the vertical
    // scan with the channels swapped.
    let scan = scan_bars(rows, yf, xf, None, agg);
    let (mut cats, mut cells, dropped) = (scan.cats, scan.cells, scan.dropped);
    if cats.is_empty() {
        return Err(no_rows_error(xf, yf));
    }
    // An explicit ordinal y sorts its categories lexically, and so does a
    // temporal one (declared DATE/TIMESTAMP, explicit spec type, or promoted
    // ISO strings) — lexical ISO order IS chronological, so a date category
    // axis stays a timeline however the rows arrive. Nominal keeps first-seen
    // (ranking) order.
    if matches!(
        resolved_type(&spec.encoding.y, table),
        FieldType::Ordinal | FieldType::Temporal
    ) {
        (cats, cells) = sort_cats(cats, cells);
    }
    // Raw per-category value counts, for --meta.
    let series_points: Vec<usize> = cells.iter().map(|r| r[0].len()).collect();
    let (agg_cells, vmax) = aggregate_cells(&cells, agg, false)?;
    let values: Vec<f64> = agg_cells
        .iter()
        .map(|r| r[0].expect("plain bars: a scanned category has values"))
        .collect();
    let xscale = Linear::nice_from(0.0, vmax, (plot_w / 10).clamp(2, 7), true);

    let n = cats.len();

    // Content-sized height: one bar row per category, one blank row between →
    // n*2 - 1. An explicit height is a CEILING (not a target); with none, a
    // safety cap of 40 rows guards against a runaway ranking.
    let content = n * 2 - 1;
    let ceiling = raw_height.unwrap_or(40);
    if content > ceiling {
        return Err(height_ceiling_error(n, content));
    }
    let plot_h = content;

    // Grouped color was routed away above, so any remaining color channel names
    // the category (y) field: a categorical tint (one color per bar).
    let categorical = spec.encoding.color.is_some();

    let (names, gutter) = name_gutter(&cats);
    let cat_scale = Linear::indices(n);

    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let total_rows = title_rows + plot_h + 2;
    let top = title_rows;

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
            x_type: None,
            time_unit: None,
            bin: None,
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

/// Grouped HORIZONTAL bars: `color` names a third field, splitting each
/// category into a stack of series rows. The transpose of `compile_bar_grouped`
/// — categories come from the Y field, series from the color field, and the
/// SERIES dimension grows down the rows (not across columns). Content-sized in
/// height like plain horizontal: each category block is `n_series` bar rows plus
/// one blank separator, so `plot_h = n_cats * (n_series + 1) - 1`, capped by the
/// raw height ceiling. A series keeps a STABLE within-block row offset in every
/// block, so a missing (category, series) cell leaves a visible empty row.
fn compile_bar_h_grouped(
    spec: &Spec,
    table: &Table,
    opts: &CompileOptions,
    plot_w: usize,
    raw_height: Option<usize>,
) -> Result<Scene, Error> {
    let rows = &table.rows;
    let xf = &spec.encoding.x.field; // value (quantitative)
    let yf = &spec.encoding.y.field; // category
    let cf = &spec
        .encoding
        .color
        .as_ref()
        .expect("grouped bars require a color channel")
        .field;
    let agg = spec.encoding.x.aggregate.unwrap_or(Aggregate::Sum);
    let theme = &opts.theme;

    // Scan: categories from the Y field, series from the color field — the
    // vertical grouped scan with the channels swapped.
    let scan = scan_bars(rows, yf, xf, Some(cf), agg);
    let (mut cats, mut raw_cells, dropped) = (scan.cats, scan.cells, scan.dropped);
    let series_names = scan.series;
    if cats.is_empty() {
        return Err(no_rows_error(xf, yf));
    }

    // An explicit ordinal y sorts its categories lexically, and so does a
    // temporal one (declared DATE/TIMESTAMP, explicit spec type, or promoted
    // ISO strings) — lexical ISO order IS chronological. Series order stays
    // first-seen.
    if matches!(
        resolved_type(&spec.encoding.y, table),
        FieldType::Ordinal | FieldType::Temporal
    ) {
        (cats, raw_cells) = sort_cats(cats, raw_cells);
    }

    let n_cats = cats.len();
    let n_series = series_names.len();

    // Palette cap (before layout): color is the only channel identifying a
    // series, so cycling the palette would make two series indistinguishable.
    if n_series > theme.palette.len() {
        return Err(palette_cap_error(n_series, theme.palette.len(), cf));
    }

    let (cells, vmax) = aggregate_cells(&raw_cells, agg, false)?;
    let xscale = Linear::nice_from(0.0, vmax, (plot_w / 10).clamp(2, 7), true);

    // Content-sized height: each category block is `n_series` bar rows plus one
    // blank separator (dropped after the last block). The raw height Option is a
    // CEILING (Some) or the 40-row safety cap (None), and the over-ceiling error
    // counts BARS (n_cats * n_series), same message as plain horizontal.
    let content = n_cats * (n_series + 1) - 1;
    let ceiling = raw_height.unwrap_or(40);
    if content > ceiling {
        return Err(height_ceiling_error(n_cats * n_series, content));
    }
    let plot_h = content;

    let (names, gutter) = name_gutter(&cats);
    let cat_scale = Linear::indices(n_cats);

    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let top = title_rows;

    // One bar per series row within a block; a series keeps its within-block
    // offset in every block. The category name centers on its block:
    // YTick row = block_start + (n_series - 1) / 2.
    let rows_f = plot_h as f64;
    let mut bars: Vec<Bar> = Vec::new();
    let mut ticks: Vec<YTick> = Vec::new();
    for (ci, cell_row) in cells.iter().enumerate() {
        let block_start = ci * (n_series + 1);
        for (si, cell) in cell_row.iter().enumerate() {
            if let Some(v) = cell {
                let plot_row = block_start + si;
                bars.push(Bar {
                    x0: 0.0,
                    y0: plot_row as f64 / rows_f,
                    w: xscale.norm(*v),
                    h: 1.0 / rows_f,
                    color: theme.series(si),
                });
            }
        }
        ticks.push(YTick {
            value: ci as f64,
            frac: cat_scale.norm(ci as f64),
            label: names[ci].clone(),
            row: top + block_start + (n_series - 1) / 2,
        });
    }

    let (legend, legend_rows) = legend_below(&series_names, theme, gutter, columns, top, plot_h);
    let total_rows = title_rows + plot_h + 2 + legend_rows;

    let (tick_cols, labels) = value_axis_x(&xscale, plot_w, gutter, columns, top + plot_h + 1);

    // Per-series cell counts: how many categories the series appears in (the
    // xy shape's points-per-series analog, matching vertical grouped).
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
        // The category axis lives on y; its "ticks" are the block-centered names,
        // and the RAW (untruncated) names ride along for the --meta surface.
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
            x_type: None,
            time_unit: None,
            bin: None,
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}
