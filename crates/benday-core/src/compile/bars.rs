//! The four bar compilers: plain and grouped, vertical and horizontal.
//! Orientation swaps which channel is the category vs. the value axis;
//! grouping adds a series dimension from the color field. All four share the
//! parent module's scan/sort/aggregate stages — each function here is the
//! orchestration plus only the layout specific to its shape.

use super::*;

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
    if is_grouped(spec.encoding.color.as_ref(), xf) {
        return compile_bar_grouped(spec, table, opts, plot_w, plot_h);
    }

    let scan = scan_bars(rows, xf, yf, None, agg);
    let (mut cats, mut cells, dropped) = (scan.cats, scan.cells, scan.dropped);
    if cats.is_empty() {
        return Err(no_rows_error(yf, xf));
    }
    // Bars always treat x as categorical; the resolved x type only decides
    // whether categories sort chronologically (ordinal DATE/TIMESTAMP or an
    // explicit ordinal spec type).
    if resolved_type(&spec.encoding.x, table) == FieldType::Ordinal {
        (cats, cells) = sort_cats(cats, cells);
    }
    // Raw per-category value counts, for --meta.
    let series_points: Vec<usize> = cells.iter().map(|r| r[0].len()).collect();
    let (agg_cells, vmax) = aggregate_cells(&cells, agg)?;
    // Plain bars have exactly one (unnamed) series, and a category only exists
    // because a row landed in it — every cell is Some.
    let values: Vec<f64> = agg_cells
        .iter()
        .map(|r| r[0].expect("plain bars: a scanned category has values"))
        .collect();
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

    // Ordinal x sorts its categories lexically (declared DATE/TIMESTAMP or an
    // explicit ordinal type); series order stays first-seen.
    if resolved_type(&spec.encoding.x, table) == FieldType::Ordinal {
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

    let (cells, vmax) = aggregate_cells(&raw_cells, agg)?;
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
    // Ordinal y (declared DATE/TIMESTAMP or an explicit ordinal spec type) sorts
    // its categories lexically; nominal keeps first-seen (ranking) order.
    if resolved_type(&spec.encoding.y, table) == FieldType::Ordinal {
        (cats, cells) = sort_cats(cats, cells);
    }
    // Raw per-category value counts, for --meta.
    let series_points: Vec<usize> = cells.iter().map(|r| r[0].len()).collect();
    let (agg_cells, vmax) = aggregate_cells(&cells, agg)?;
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

    // Ordinal y (declared DATE/TIMESTAMP or an explicit ordinal type) sorts its
    // categories lexically; series order stays first-seen.
    if resolved_type(&spec.encoding.y, table) == FieldType::Ordinal {
        (cats, raw_cells) = sort_cats(cats, raw_cells);
    }

    let n_cats = cats.len();
    let n_series = series_names.len();

    // Palette cap (before layout): color is the only channel identifying a
    // series, so cycling the palette would make two series indistinguishable.
    if n_series > theme.palette.len() {
        return Err(palette_cap_error(n_series, theme.palette.len(), cf));
    }

    let (cells, vmax) = aggregate_cells(&raw_cells, agg)?;
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
            series_points,
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}
