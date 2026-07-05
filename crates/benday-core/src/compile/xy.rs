//! The line/point/area compiler: per-series normalized points on shared
//! scales, with the y fraction flipped here (see the parent module docs).

use super::*;

/// One resolved line/point/area series: display name plus raw (x, y) points.
struct XySeries {
    name: String,
    points: Vec<(f64, f64)>,
}

/// Compile line/point/area marks: split into series, sort, build scales, and
/// normalize each point to `[frac_x, 1 - frac_y]` (frac_y flipped here — see
/// the module docs). One `SceneMark` per series, in first-seen order.
pub(super) fn compile_xy(
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

    let xt = resolved_type(&spec.encoding.x, table);
    let yt = resolved_type(&spec.encoding.y, table);
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
        return Err(palette_cap_error(series.len(), theme.palette.len(), cf));
    }

    // --- Layout: optional title row above the plot, y gutter to its left,
    // legend below the x labels.
    let gutter = tick_gutter(&yscale);
    let columns = gutter + 1 + plot_w;
    let (title, title_rows) = place_title(spec, gutter, plot_w);
    let top = title_rows;

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
