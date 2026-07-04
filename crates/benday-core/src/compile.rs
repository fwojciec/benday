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

use std::collections::HashSet;

use crate::data;
use crate::error::Error;
use crate::scale::{fmt_tick, Linear};
use crate::scene::{
    Bar, Chrome, LegendEntry, Placed, Rect, Scene, SceneMark, SeriesRef, Size, Source, XAxis,
    YAxis, YTick,
};
use crate::spec::{Aggregate, FieldType, Mark, Spec};
use crate::theme::Theme;

const DEFAULT_WIDTH: usize = 60;
const DEFAULT_HEIGHT: usize = 10;

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

/// Spec- and data-level rules the type system can't express, run before either
/// render path. Loud by design: a silently ignored channel produces a chart
/// the caller didn't ask for, which an agent reading dot art cannot detect.
pub fn preflight(spec: &Spec) -> Result<(), Error> {
    validate(spec)?;
    let rows = &spec.data.values;
    if rows.is_empty() {
        return Err(Error::Data(
            "`data.values` is empty; provide at least one row of objects".into(),
        ));
    }
    data::check_field(rows, &spec.encoding.x.field)?;
    if !matches!(spec.encoding.y.aggregate, Some(Aggregate::Count)) {
        data::check_field(rows, &spec.encoding.y.field)?;
    }
    if let Some(c) = &spec.encoding.color {
        data::check_field(rows, &c.field)?;
    }
    Ok(())
}

fn validate(spec: &Spec) -> Result<(), Error> {
    if spec.encoding.x.aggregate.is_some() {
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
    if spec.mark == Mark::Bar {
        if let Some(c) = &spec.encoding.color {
            if c.field != spec.encoding.x.field {
                return Err(Error::Spec(format!(
                    "bar marks cannot group by color yet; encoding.color.field must equal \
                     encoding.x.field (\"{}\") or be omitted",
                    spec.encoding.x.field
                )));
            }
        }
        if spec.encoding.x.ty == Some(FieldType::Quantitative) {
            return Err(Error::Spec(
                "bar marks treat x as categorical; omit encoding.x.type, or use mark \"line\" \
                 or \"area\" for a quantitative x"
                    .into(),
            ));
        }
    }
    Ok(())
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
pub fn compile(spec: &Spec, opts: &CompileOptions) -> Result<Scene, Error> {
    preflight(spec)?;
    let (plot_w, plot_h) = plot_dims(opts.width, opts.height, spec);
    match spec.mark {
        Mark::Bar => compile_bar(spec, opts, plot_w, plot_h),
        Mark::Line | Mark::Point | Mark::Area => compile_xy(spec, opts, plot_w, plot_h),
    }
}

fn compile_bar(
    spec: &Spec,
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &spec.data.values;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let agg = spec.encoding.y.aggregate.unwrap_or(Aggregate::Sum);
    let theme = &opts.theme;

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
    let values: Vec<f64> = groups.iter().map(|g| aggregate(g, agg)).collect();
    if values.iter().any(|v| *v < 0.0) {
        return Err(Error::Data(
            "negative values are not yet supported for mark \"bar\"; use mark \"line\"".into(),
        ));
    }
    let vmax = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y = Linear::nice_from(0.0, vmax, plot_h.clamp(3, 6), true);
    // validate() guarantees a color channel, if present, encodes the x field.
    let categorical = spec.encoding.color.is_some();

    // --- Layout, formerly split across Frame::new / draw_y_axis / draw_x_axis.
    let title_rows = usize::from(spec.title.is_some());
    // Bars carry no legend (render_bar always passed an empty legend).
    let legend_rows = 0usize;
    let gutter = y
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, y.step).chars().count())
        .max()
        .unwrap_or(1);
    let columns = gutter + 1 + plot_w;
    let total_rows = title_rows + legend_rows + plot_h + 2;
    let top = title_rows + legend_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    // Y ticks: buffer-absolute rows, first-wins on collision.
    let mut used = HashSet::new();
    let mut ticks: Vec<YTick> = Vec::new();
    for t in y.ticks() {
        let r = ((1.0 - y.norm(t)) * (plot_h - 1) as f64).round() as usize;
        if !used.insert(r) {
            continue;
        }
        ticks.push(YTick {
            value: t,
            frac: y.norm(t),
            label: fmt_tick(t, y.step),
            row: top + r,
        });
    }

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
            ticks,
        },
        x_axis: XAxis {
            categories: Some(cats),
            domain: None,
            tick_cols: Vec::new(),
            labels,
        },
        marks: vec![SceneMark::Bars { bars }],
        dropped_rows: dropped,
        source: Source {
            mark: Mark::Bar,
            x_field: xf.clone(),
            y_field: yf.clone(),
            aggregate: Some(agg),
            series_points: groups.iter().map(Vec::len).collect(),
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
    opts: &CompileOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Scene, Error> {
    let rows = &spec.data.values;
    let xf = &spec.encoding.x.field;
    let yf = &spec.encoding.y.field;
    let theme = &opts.theme;
    let mark = spec.mark;

    let xt = spec
        .encoding
        .x
        .ty
        .unwrap_or_else(|| data::infer_type(rows, xf));
    let yt = spec
        .encoding
        .y
        .ty
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
    let yscale = Linear::nice_from(ymin, ymax, plot_h.clamp(3, 6), mark == Mark::Area);
    let xscale = if xt == FieldType::Quantitative {
        Linear::nice_from(xmin, xmax, (plot_w / 10).clamp(2, 7), false)
    } else {
        Linear::indices(x_cats.len())
    };

    let multi = series.len() > 1 || series_field.is_some();

    // --- Layout: title/legend rows above the plot, y gutter to its left.
    let title_rows = usize::from(spec.title.is_some());
    let legend_rows = usize::from(multi);
    let gutter = yscale
        .ticks()
        .iter()
        .map(|t| fmt_tick(*t, yscale.step).chars().count())
        .max()
        .unwrap_or(1);
    let columns = gutter + 1 + plot_w;
    let total_rows = title_rows + legend_rows + plot_h + 2;
    let top = title_rows + legend_rows;

    let title = spec.title.as_deref().map(|t| {
        let col = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
        Placed {
            text: t.to_string(),
            col,
            row: 0,
        }
    });

    // Legend (multi-series only): "── name" entries advancing 3 + len + 3.
    let mut legend: Vec<LegendEntry> = Vec::new();
    if multi {
        let mut x = gutter + 1;
        for (i, s) in series.iter().enumerate() {
            let len = s.name.chars().count();
            legend.push(LegendEntry {
                name: s.name.clone(),
                color: theme.series(i),
                col: x,
                row: title_rows,
            });
            x += 3 + len + 3;
        }
    }

    // Y ticks: buffer-absolute rows, first-wins on collision.
    let mut used = HashSet::new();
    let mut ticks: Vec<YTick> = Vec::new();
    for t in yscale.ticks() {
        let r = ((1.0 - yscale.norm(t)) * (plot_h - 1) as f64).round() as usize;
        if !used.insert(r) {
            continue;
        }
        ticks.push(YTick {
            value: t,
            frac: yscale.norm(t),
            label: fmt_tick(t, yscale.step),
            row: top + r,
        });
    }

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

    // X axis: tick columns (plot-relative) + label anchors, then greedy layout.
    let label_row = top + plot_h + 1;
    let (tick_cols, anchors): (Vec<usize>, Vec<(usize, String)>) = if xt == FieldType::Quantitative
    {
        let tks = xscale.ticks();
        let cols: Vec<usize> = tks
            .iter()
            .map(|t| ((xscale.norm(*t) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1))
            .collect();
        let anchors = cols
            .iter()
            .zip(&tks)
            .map(|(c, t)| (*c, fmt_tick(*t, xscale.step)))
            .collect();
        (cols, anchors)
    } else {
        let cols: Vec<usize> = (0..x_cats.len())
            .map(|i| {
                ((xscale.norm(i as f64) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1)
            })
            .collect();
        let anchors = cols
            .iter()
            .zip(&x_cats)
            .map(|(c, name)| (*c, truncate(name, 12)))
            .collect();
        (cols, anchors)
    };
    let labels = place_x_labels(&anchors, gutter, columns, label_row);

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
        },
    })
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
