//! The render pipeline: validate the spec, resolve encodings against data,
//! build scales, solve layout, draw marks, serialize to ANSI.

use std::collections::HashSet;

use serde_json::json;

use crate::ansi::Buffer;
use crate::data;
use crate::error::Error;
use crate::raster::{Marker, PixelCanvas, Rgb};
use crate::scale::{fmt_tick, Linear};
use crate::spec::{Aggregate, FieldType, Mark, Spec};
use crate::theme::Theme;

pub struct RenderOptions {
    /// Plot area width in cells; overrides spec.width.
    pub width: Option<usize>,
    /// Plot area height in cells; overrides spec.height.
    pub height: Option<usize>,
    pub marker: Marker,
    pub bar_style: BarStyle,
    pub theme: Theme,
    pub color: bool,
}

/// How bar marks are filled. Dots (the house style) rasterize bars through
/// the pixel canvas at 4 vertical levels per cell; blocks give a solid
/// silhouette with finer 8-levels-per-cell caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarStyle {
    Dots,
    Blocks,
}

#[derive(Debug)]
pub struct Rendered {
    pub text: String,
    pub meta: serde_json::Value,
}

const DEFAULT_WIDTH: usize = 60;
const DEFAULT_HEIGHT: usize = 10;
const EIGHTHS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

pub fn render(spec: &Spec, opts: &RenderOptions) -> Result<Rendered, Error> {
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

    let plot_w = opts.width.or(spec.width).unwrap_or(DEFAULT_WIDTH).max(8);
    let plot_h = opts.height.or(spec.height).unwrap_or(DEFAULT_HEIGHT).max(3);

    match spec.mark {
        Mark::Bar => render_bar(spec, opts, plot_w, plot_h),
        Mark::Line | Mark::Point | Mark::Area => render_xy(spec, opts, plot_w, plot_h),
    }
}

/// Spec-level rules the type system can't express. Loud by design: a
/// silently ignored channel produces a chart the caller didn't ask for,
/// which an agent reading dot art cannot detect.
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

fn aggregate(values: &[f64], agg: Aggregate) -> f64 {
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// One chart's assembled layout: title and legend rows above a plot area
/// with a y-axis gutter to its left and an x axis plus label row below.
/// Owns the cell buffer; mark renderers draw into `plot area` coordinates
/// via `blit` (pixel canvases) or `buf` directly (block bars).
struct Frame {
    buf: Buffer,
    gutter: usize,
    /// First row of the plot area.
    top: usize,
    plot_w: usize,
    plot_h: usize,
}

impl Frame {
    fn new(
        title: Option<&str>,
        legend: &[(String, Rgb)],
        yscale: &Linear,
        plot_w: usize,
        plot_h: usize,
        theme: &Theme,
    ) -> Frame {
        let gutter = yscale
            .ticks()
            .iter()
            .map(|t| fmt_tick(*t, yscale.step).chars().count())
            .max()
            .unwrap_or(1);
        let title_rows = usize::from(title.is_some());
        let legend_rows = usize::from(!legend.is_empty());
        let mut buf = Buffer::new(gutter + 1 + plot_w, title_rows + legend_rows + plot_h + 2);

        if let Some(t) = title {
            let start = gutter + 1 + plot_w.saturating_sub(t.chars().count()) / 2;
            buf.text(start, 0, t, Some(theme.title));
        }
        if !legend.is_empty() {
            let mut x = gutter + 1;
            for (name, color) in legend {
                buf.text(x, title_rows, "──", Some(*color));
                buf.text(x + 3, title_rows, name, Some(theme.axis));
                x += 3 + name.chars().count() + 3;
            }
        }

        let mut frame = Frame {
            buf,
            gutter,
            top: title_rows + legend_rows,
            plot_w,
            plot_h,
        };
        frame.draw_y_axis(yscale, theme);
        frame
    }

    fn draw_y_axis(&mut self, scale: &Linear, theme: &Theme) {
        for r in 0..self.plot_h {
            self.buf
                .set(self.gutter, self.top + r, '│', Some(theme.axis));
        }
        let mut used = HashSet::new();
        for t in scale.ticks() {
            let r = ((1.0 - scale.norm(t)) * (self.plot_h - 1) as f64).round() as usize;
            if !used.insert(r) {
                continue;
            }
            let label = fmt_tick(t, scale.step);
            let len = label.chars().count();
            self.buf
                .set(self.gutter, self.top + r, '┤', Some(theme.axis));
            self.buf.text(
                self.gutter.saturating_sub(len),
                self.top + r,
                &label,
                Some(theme.axis),
            );
        }
    }

    /// Copy a pixel canvas into the plot area.
    fn blit(&mut self, canvas: &PixelCanvas) {
        for cy in 0..self.plot_h {
            for cx in 0..self.plot_w {
                if let Some((ch, color)) = canvas.cell(cx, cy) {
                    self.buf
                        .set(self.gutter + 1 + cx, self.top + cy, ch, Some(color));
                }
            }
        }
    }

    /// Axis line with ticks, then labels placed greedily (centered on their
    /// column, skipped on collision). Columns are plot-area-relative.
    fn draw_x_axis(&mut self, tick_cols: &[usize], labels: &[(usize, String)], theme: &Theme) {
        let row = self.top + self.plot_h;
        self.buf.set(self.gutter, row, '└', Some(theme.axis));
        for c in 0..self.plot_w {
            self.buf
                .set(self.gutter + 1 + c, row, '─', Some(theme.axis));
        }
        for &c in tick_cols {
            if c < self.plot_w {
                self.buf
                    .set(self.gutter + 1 + c, row, '┴', Some(theme.axis));
            }
        }
        let width = self.buf.width();
        let mut next_free = 0usize;
        for (col, label) in labels {
            let len = label.chars().count();
            if len == 0 || len > width {
                continue;
            }
            let start = (self.gutter + 1 + col)
                .saturating_sub(len / 2)
                .min(width - len);
            if start < next_free {
                continue;
            }
            self.buf.text(start, row + 1, label, Some(theme.axis));
            next_free = start + len + 1;
        }
    }

    fn size_meta(&self) -> serde_json::Value {
        json!({ "columns": self.buf.width(), "rows": self.buf.height() })
    }

    fn finish(self, color: bool) -> String {
        self.buf.to_ansi(color)
    }
}

fn render_bar(
    spec: &Spec,
    opts: &RenderOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Rendered, Error> {
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

    let mut frame = Frame::new(spec.title.as_deref(), &[], &y, plot_w, plot_h, theme);
    let mut canvas = PixelCanvas::new(plot_w, plot_h, opts.marker);

    let n = cats.len();
    let step = plot_w as f64 / n as f64;
    let bar_w = ((step * 0.7).floor() as usize).clamp(1, plot_w);
    let label_max = (step.floor() as usize).saturating_sub(1).max(1);

    let mut x_labels: Vec<(usize, String)> = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let center = (i as f64 + 0.5) * step;
        let x0 = ((center - bar_w as f64 / 2.0).round().max(0.0) as usize).min(plot_w - bar_w);
        let color = if categorical {
            theme.series(i)
        } else {
            theme.grad(y.norm(*v))
        };
        match opts.bar_style {
            BarStyle::Blocks => {
                let level = (y.norm(*v) * (plot_h * 8) as f64).round() as i64;
                for r in 0..plot_h {
                    let fill = level - ((plot_h - 1 - r) * 8) as i64;
                    if fill <= 0 {
                        continue;
                    }
                    let ch = if fill >= 8 {
                        '█'
                    } else {
                        EIGHTHS[fill as usize]
                    };
                    for c in 0..bar_w {
                        frame
                            .buf
                            .set(frame.gutter + 1 + x0 + c, frame.top + r, ch, Some(color));
                    }
                }
            }
            BarStyle::Dots => {
                let ph = (plot_h * 4) as i64;
                let level = (y.norm(*v) * ph as f64).round() as i64;
                for px in (x0 * 2) as i64..((x0 + bar_w) * 2) as i64 {
                    for py in (ph - level)..ph {
                        canvas.set(px, py, color);
                    }
                }
            }
        }
        x_labels.push((
            (center.round() as usize).min(plot_w - 1),
            truncate(&cats[i], label_max),
        ));
    }
    if opts.bar_style == BarStyle::Dots {
        frame.blit(&canvas);
    }
    frame.draw_x_axis(&[], &x_labels, theme);

    let meta = json!({
        "mark": "bar",
        "x": { "field": xf, "type": "nominal", "categories": cats },
        "y": { "field": yf, "aggregate": agg, "domain": [y.min, y.max] },
        "dropped_rows": dropped,
        "size": frame.size_meta(),
    });
    Ok(Rendered {
        text: frame.finish(opts.color),
        meta,
    })
}

struct Series {
    name: String,
    points: Vec<(f64, f64)>,
}

fn render_xy(
    spec: &Spec,
    opts: &RenderOptions,
    plot_w: usize,
    plot_h: usize,
) -> Result<Rendered, Error> {
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

    let mut series: Vec<Series> = Vec::new();
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
                series.push(Series {
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

    let mut canvas = PixelCanvas::new(plot_w, plot_h, opts.marker);
    let (pw, ph) = (canvas.pixel_width() as i64, canvas.pixel_height() as i64);
    let px = |v: f64| (xscale.norm(v) * (pw - 1) as f64).round() as i64;
    let py = |v: f64| ((1.0 - yscale.norm(v)) * (ph - 1) as f64).round() as i64;

    let multi = series.len() > 1 || series_field.is_some();
    for (si, s) in series.iter().enumerate() {
        let color = if multi {
            theme.series(si)
        } else {
            theme.accent
        };
        match mark {
            Mark::Point => {
                for (x, y) in &s.points {
                    let (cx, cy) = (px(*x), py(*y));
                    for dx in 0..2 {
                        for dy in 0..2 {
                            canvas.set(cx + dx, cy + dy, color);
                        }
                    }
                }
            }
            Mark::Line | Mark::Area => {
                if mark == Mark::Area {
                    for w in s.points.windows(2) {
                        let (x0, y0, x1, y1) = (px(w[0].0), py(w[0].1), px(w[1].0), py(w[1].1));
                        for x in x0..=x1 {
                            let t = if x1 == x0 {
                                0.0
                            } else {
                                (x - x0) as f64 / (x1 - x0) as f64
                            };
                            let ytop = (y0 as f64 + t * (y1 - y0) as f64).round() as i64;
                            for yy in ytop..ph {
                                canvas.set(x, yy, color);
                            }
                        }
                    }
                }
                if s.points.len() == 1 {
                    let (x, y) = s.points[0];
                    canvas.set(px(x), py(y), color);
                    canvas.set(px(x) + 1, py(y), color);
                }
                for w in s.points.windows(2) {
                    canvas.line(px(w[0].0), py(w[0].1), px(w[1].0), py(w[1].1), color);
                }
            }
            Mark::Bar => unreachable!("bar handled by render_bar"),
        }
    }

    let legend: Vec<(String, Rgb)> = if multi {
        series
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), theme.series(i)))
            .collect()
    } else {
        Vec::new()
    };
    let mut frame = Frame::new(
        spec.title.as_deref(),
        &legend,
        &yscale,
        plot_w,
        plot_h,
        theme,
    );
    frame.blit(&canvas);

    let (tick_cols, x_labels): (Vec<usize>, Vec<(usize, String)>) = if xt == FieldType::Quantitative
    {
        let ticks = xscale.ticks();
        let cols: Vec<usize> = ticks
            .iter()
            .map(|t| ((xscale.norm(*t) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1))
            .collect();
        let labels = cols
            .iter()
            .zip(&ticks)
            .map(|(c, t)| (*c, fmt_tick(*t, xscale.step)))
            .collect();
        (cols, labels)
    } else {
        let cols: Vec<usize> = (0..x_cats.len())
            .map(|i| {
                ((xscale.norm(i as f64) * (plot_w - 1) as f64).round() as usize).min(plot_w - 1)
            })
            .collect();
        let labels = cols
            .iter()
            .zip(&x_cats)
            .map(|(c, name)| (*c, truncate(name, 12)))
            .collect();
        (cols, labels)
    };
    frame.draw_x_axis(&tick_cols, &x_labels, theme);

    let meta = json!({
        "mark": mark,
        "x": {
            "field": xf,
            "type": if xt == FieldType::Quantitative { "quantitative" } else { "nominal" },
            "domain": if xt == FieldType::Quantitative {
                json!([xscale.min, xscale.max])
            } else {
                json!(x_cats)
            },
        },
        "y": { "field": yf, "domain": [yscale.min, yscale.max] },
        "series": series
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let color = if multi { theme.series(i) } else { theme.accent };
                json!({ "name": s.name, "color": color.hex(), "points": s.points.len() })
            })
            .collect::<Vec<_>>(),
        "dropped_rows": dropped,
        "size": frame.size_meta(),
    });
    Ok(Rendered {
        text: frame.finish(opts.color),
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RenderOptions {
        RenderOptions {
            width: None,
            height: None,
            marker: Marker::Braille,
            bar_style: BarStyle::Dots,
            theme: crate::theme::by_name("benday").unwrap(),
            color: false,
        }
    }

    fn spec(json: &str) -> Spec {
        serde_json::from_str(json).unwrap()
    }

    fn is_braille(c: char) -> bool {
        ('\u{2800}'..='\u{28FF}').contains(&c)
    }

    #[test]
    fn bar_chart_dots_by_default() {
        let s = spec(
            r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7}]},
                "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        );
        let out = render(&s, &opts()).unwrap();
        assert!(out.text.chars().any(is_braille));
        assert!(out.text.contains("jan"));
    }

    #[test]
    fn bar_chart_blocks_style() {
        let s = spec(
            r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7}]},
                "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        );
        let out = render(
            &s,
            &RenderOptions {
                bar_style: BarStyle::Blocks,
                ..opts()
            },
        )
        .unwrap();
        assert!(out.text.contains('█'));
    }

    #[test]
    fn line_chart_smoke() {
        let s = spec(
            r#"{"data":{"values":[{"x":0,"y":1},{"x":1,"y":4},{"x":2,"y":2}]},
                "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        );
        let out = render(&s, &opts()).unwrap();
        assert!(out.text.chars().any(is_braille));
    }

    #[test]
    fn missing_field_is_actionable() {
        let s = spec(
            r#"{"data":{"values":[{"month":"jan","sales":3}]},
                "mark":"bar","encoding":{"x":{"field":"month"},"y":{"field":"revenue"}}}"#,
        );
        let err = render(&s, &opts()).unwrap_err();
        let msg = err.to_string();
        assert_eq!(err.kind(), "data");
        assert!(msg.contains("revenue") && msg.contains("available fields"));
        assert!(msg.contains("month") && msg.contains("sales"));
    }

    #[test]
    fn aggregate_on_x_is_rejected() {
        let s = spec(
            r#"{"data":{"values":[{"m":"jan","v":3}]},
                "mark":"bar",
                "encoding":{"x":{"field":"m","aggregate":"sum"},"y":{"field":"v"}}}"#,
        );
        let err = render(&s, &opts()).unwrap_err();
        assert_eq!(err.kind(), "spec");
        assert!(err.to_string().contains("encoding.x"));
    }

    #[test]
    fn bar_color_grouping_is_rejected_not_ignored() {
        let s = spec(
            r#"{"data":{"values":[{"m":"jan","v":3,"region":"west"}]},
                "mark":"bar",
                "encoding":{"x":{"field":"m"},"y":{"field":"v"},"color":{"field":"region"}}}"#,
        );
        let err = render(&s, &opts()).unwrap_err();
        assert_eq!(err.kind(), "spec");
        assert!(err.to_string().contains("color"));
    }
}
