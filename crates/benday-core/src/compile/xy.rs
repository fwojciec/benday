//! The line/point/area compiler: per-series normalized points on shared
//! scales, with the y fraction flipped here (see the parent module docs).

use super::*;
use crate::time;

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
    // Temporal y has no use case and would otherwise hit the generic
    // categorical-y message; reject it with a fix that names the resolved type.
    if yt == FieldType::Temporal {
        return Err(temporal_y_error(mark, yf));
    }
    if yt != FieldType::Quantitative {
        return Err(Error::Data(format!(
            "mark {mark:?} needs a quantitative y, but field \"{yf}\" holds categorical values; \
             put categories on x, or set encoding.y.type to \"quantitative\" if they are numbers"
        )));
    }
    // x is CONTINUOUS for quantitative and temporal alike; the two-way
    // classification threads through all four forks below (row parsing, scale,
    // axis, domain). Within the continuous arm, temporal differs in exactly
    // three places: the value reader, the scale, and the label formatter.
    let x_cont = matches!(xt, FieldType::Quantitative | FieldType::Temporal);

    let series_field = spec
        .encoding
        .color
        .as_ref()
        .map(|c| c.field.clone())
        .filter(|f| f != xf);
    // Temporal color would explode into one series per timestamp; reject before
    // the scan (only when color is a genuine series field, i.e. not the x field).
    if let Some(c) = &spec.encoding.color {
        if series_field.is_some() && resolved_type(c, table) == FieldType::Temporal {
            return Err(temporal_color_error(&c.field));
        }
    }

    let mut series: Vec<XySeries> = Vec::new();
    let mut x_cats: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for (ri, row) in rows.iter().enumerate() {
        let (Some(xv), Some(yv)) = (row.get(xf), row.get(yf)) else {
            dropped += 1;
            continue;
        };
        let Some(yn) = data::num(yv) else {
            dropped += 1;
            continue;
        };
        let xn = if x_cont {
            if xt == FieldType::Temporal {
                // Null is not an unparseable string: it drops like the
                // quantitative arm's (and `infer_type` skips nulls when
                // promoting, so a PROMOTED column can carry one). With nulls
                // dropped, a promoted column's values parse by construction —
                // the hard error fires only for declared/explicit intent
                // meeting non-null garbage, and names the value.
                if xv.is_null() {
                    dropped += 1;
                    continue;
                }
                match time::parse_temporal(&data::text(xv)) {
                    Some(v) => v,
                    None => return Err(temporal_parse_error(ri, &data::text(xv), xf)),
                }
            } else {
                match data::num(xv) {
                    Some(v) => v,
                    None => {
                        dropped += 1;
                        continue;
                    }
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
    // The temporal axis (calendar ladder ticks + boundary-expanded domain) is
    // computed once and reused by the scale and axis forks below.
    let temporal = (xt == FieldType::Temporal).then(|| time::temporal_axis(xmin, xmax, plot_w));
    let xscale = if x_cont {
        match &temporal {
            Some(ta) => {
                // Feed the temporal domain into a Linear so `norm` and the scene
                // contract are unchanged. A single-instant series yields a
                // zero-width domain [x, x] whose `norm` is NaN; expand it exactly
                // as `nice_from` does for a degenerate quantitative span.
                let (lo, mut hi) = (ta.domain[0], ta.domain[1]);
                if !(hi - lo).is_normal() {
                    hi = lo + 1.0;
                }
                Linear {
                    min: lo,
                    max: hi,
                    step: hi - lo,
                }
            }
            None => Linear::nice_from(xmin, xmax, (plot_w / 10).clamp(2, 7), false),
        }
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
    let (tick_cols, labels): (Vec<usize>, Vec<Placed>) = if x_cont {
        match &temporal {
            Some(ta) => {
                // Temporal ticks sit on calendar boundaries (from `temporal_axis`).
                // Map each (ms, label) to a column with the shared `x_col`
                // arithmetic, then place the pre-formed calendar labels
                // greedily — NOT `value_axis_x`, whose `fmt_tick` is numeric.
                let cols: Vec<usize> = ta
                    .ticks
                    .iter()
                    .map(|(ms, _)| x_col(&xscale, *ms, plot_w))
                    .collect();
                let anchors: Vec<(usize, String)> = cols
                    .iter()
                    .zip(&ta.ticks)
                    .map(|(c, (_, label))| (*c, label.clone()))
                    .collect();
                let labels = place_x_labels(&anchors, gutter, columns, label_row);
                (cols, labels)
            }
            None => value_axis_x(&xscale, plot_w, gutter, columns, label_row),
        }
    } else {
        let cols: Vec<usize> = (0..x_cats.len())
            .map(|i| x_col(&xscale, i as f64, plot_w))
            .collect();
        let anchors: Vec<(usize, String)> = cols
            .iter()
            .zip(&x_cats)
            .map(|(c, name)| (*c, truncate(name, 12)))
            .collect();
        let labels = place_x_labels(&anchors, gutter, columns, label_row);
        (cols, labels)
    };

    let (categories, domain) = if x_cont {
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
            // Set ONLY on the temporal path so `meta()` reports an ISO domain;
            // None everywhere else keeps existing snapshots byte-identical.
            x_type: temporal.as_ref().map(|_| FieldType::Temporal),
            // timeUnit is bar-only (rejected on xy in `validate`), so never set here.
            time_unit: None,
            // bin is bar-only (rejected on xy in `validate_bin`), so never set here.
            bin: None,
            series_points: series.iter().map(|s| s.points.len()).collect(),
            data_source: table.provenance.source,
            truncated: table.provenance.truncated,
            total_rows: table.provenance.total_rows,
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::compile::{compile, CompileOptions};
    use crate::ingest;
    use crate::scene::{Scene, SceneMark};
    use crate::spec::Spec;
    use crate::theme;

    fn compile_spec(json: &str) -> Scene {
        let spec: Spec = serde_json::from_str(json).expect("spec parses");
        let table = ingest::resolve(&spec, None).expect("resolves");
        let opts = CompileOptions {
            width: None,
            height: None,
            theme: theme::by_name("benday").unwrap(),
        };
        compile(&spec, &table, &opts).expect("compiles")
    }

    #[test]
    fn temporal_ticks_all_keep_labels_under_y_gutter() {
        // End-to-end reconciliation of `time::accept` (which pre-tests label
        // placement at gutter 0) with `place_x_labels` (which runs at the real
        // y gutter): thousands-scale y forces a multi-char gutter, and every
        // accepted temporal tick must still carry its label downstream. The
        // five Mondays land exactly on week boundaries, so the rung emits five
        // ticks with no domain expansion.
        let scene = compile_spec(
            r#"{"data":{"columns":[{"name":"t","type":"DATE"},{"name":"n","type":"INT64"}],
                 "rows":[["2026-01-05",1200],["2026-01-12",3400],["2026-01-19",900],
                         ["2026-01-26",2600],["2026-02-02",1800]]},
               "mark":"line","encoding":{"x":{"field":"t"},"y":{"field":"n"}}}"#,
        );
        assert!(
            scene.plot.x >= 3,
            "expected a multi-char y gutter, got plot.x={}",
            scene.plot.x
        );
        assert!(
            scene.x_axis.tick_cols.len() >= 3,
            "expected a real tick rung, got {} ticks",
            scene.x_axis.tick_cols.len()
        );
        assert_eq!(
            scene.x_axis.labels.len(),
            scene.x_axis.tick_cols.len(),
            "every temporal tick must keep its label under a nonzero y gutter"
        );
    }

    #[test]
    fn single_point_temporal_normalizes_without_nan() {
        // A one-instant temporal series yields a zero-width domain [x, x]; the
        // guard expands it (mirroring `nice_from`) so `norm` is 0, never NaN.
        let scene = compile_spec(
            r#"{"data":{"columns":[{"name":"t","type":"DATE"},{"name":"v","type":"INT64"}],
                 "rows":[["2026-06-14",7]]},
               "mark":"line","encoding":{"x":{"field":"t"},"y":{"field":"v"}}}"#,
        );
        let SceneMark::Path { points, .. } = &scene.marks[0] else {
            panic!("expected a path mark");
        };
        assert_eq!(points.len(), 1);
        assert!(
            points[0][0].is_finite(),
            "x frac must be finite, got {}",
            points[0][0]
        );
        assert_eq!(
            points[0][0], 0.0,
            "single temporal point sits at the left edge"
        );
        let d = scene.x_axis.domain.expect("temporal x carries a domain");
        assert!(d[1] > d[0], "degenerate domain must be expanded, got {d:?}");
    }

    #[test]
    fn temporal_null_x_drops_like_quantitative() {
        // Null is not an unparseable string: `infer_type` skips nulls when
        // promoting, so a PROMOTED column can carry one — it must take the
        // quantitative arm's dropped-row path, not the hard parse error
        // (which is for non-null garbage under declared/explicit intent).
        let scene = compile_spec(
            r#"{"data":{"values":[{"d":"2026-01-05","v":3},{"d":null,"v":7},
                                  {"d":"2026-01-08","v":5}]},
               "mark":"line","encoding":{"x":{"field":"d"},"y":{"field":"v"}}}"#,
        );
        assert_eq!(scene.dropped_rows, 1, "the null row drops, silently");
        let SceneMark::Path { points, .. } = &scene.marks[0] else {
            panic!("expected a path mark");
        };
        assert_eq!(points.len(), 2, "the two non-null rows survive");
    }
}
