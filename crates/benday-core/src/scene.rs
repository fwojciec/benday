//! The Scene: benday's intermediate representation. `compile()` resolves a
//! spec against its data into a Scene — every data- and layout-dependent
//! decision made, geometry normalized to the plot rect — and `rasterize()`
//! turns a Scene into glyphs. The serialized form is the golden-corpus
//! snapshot and the `--dump-scene` output; it is explicitly unstable.

use serde::Serialize;
use serde_json::json;

use crate::ingest::DataSource;
use crate::raster::Rgb;
use crate::spec::{Aggregate, FieldType, Mark, TimeUnit};

#[derive(Serialize)]
pub struct Scene {
    pub size: Size,
    pub plot: Rect,
    /// Resolved theme colors for non-mark elements. Colors are compile-time
    /// facts everywhere — the rasterizer never sees a Theme.
    pub chrome: Chrome,
    pub title: Option<Placed>,
    pub legend: Vec<LegendEntry>,
    pub y_axis: YAxis,
    pub x_axis: XAxis,
    pub marks: Vec<SceneMark>,
    pub dropped_rows: usize,
    /// Provenance for --meta output.
    pub source: Source,
}

/// Colors for axes/labels (`axis`) and the title (`title`). Legend swatches
/// carry their own color per entry; legend NAME text uses `axis`.
#[derive(Serialize)]
pub struct Chrome {
    pub axis: Rgb,
    pub title: Rgb,
}

#[derive(Serialize)]
pub struct Size {
    pub columns: usize,
    pub rows: usize,
}

#[derive(Serialize)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Text plus its resolved starting column (buffer-absolute) and row.
#[derive(Serialize)]
pub struct Placed {
    pub text: String,
    pub col: usize,
    pub row: usize,
}

#[derive(Serialize)]
pub struct LegendEntry {
    pub name: String,
    pub color: Rgb,
    pub col: usize,
    pub row: usize,
}

#[derive(Serialize)]
pub struct YAxis {
    pub domain: [f64; 2],
    pub step: f64,
    /// Categorical y (horizontal bars): the RAW, untruncated category names in
    /// axis order — the machine-readable surface for `--meta`, where the
    /// truncated tick labels would silently corrupt names a caller matches
    /// back to its rows. None on every quantitative-y path; skipped when None
    /// so pre-existing scene snapshots stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// In draw order; rows are distinct by construction. `row` is buffer-absolute.
    pub ticks: Vec<YTick>,
}

#[derive(Serialize)]
pub struct YTick {
    pub value: f64,
    pub frac: f64,
    pub label: String,
    pub row: usize,
}

#[derive(Serialize)]
pub struct XAxis {
    /// Nominal x: resolved category order. Quantitative: None.
    pub categories: Option<Vec<String>>,
    pub domain: Option<[f64; 2]>,
    /// Columns (plot-relative) that get a '┴' glyph. Empty for bars.
    pub tick_cols: Vec<usize>,
    /// Labels that survived greedy placement; `col` is the buffer-absolute
    /// start column. Dropped labels simply don't appear — visible in diffs.
    pub labels: Vec<Placed>,
}

#[derive(Serialize)]
pub struct SeriesRef {
    pub name: Option<String>,
    pub color: Rgb,
}

#[derive(Serialize)]
pub enum SceneMark {
    Bars {
        /// One entry per category, in category order.
        bars: Vec<Bar>,
        /// Bar orientation. Rect anchors can't encode it: a bottom-row
        /// horizontal bar has `x0 == 0` AND `y0 + h == 1`, exactly a vertical
        /// bar's signature — so the direction is carried once per mark.
        direction: BarDirection,
    },
    Path {
        series: SeriesRef,
        points: Vec<[f64; 2]>,
    },
    Points {
        series: SeriesRef,
        points: Vec<[f64; 2]>,
    },
    /// Area: fill under the path plus the path itself.
    Fill {
        series: SeriesRef,
        points: Vec<[f64; 2]>,
    },
}

/// One bar as a normalized rect over the plot area: `x0/w` as fractions of
/// plot width, `y0/h` as fractions of plot height, y0 = 0 at the TOP (same
/// orientation as point geometry). Vertical bars: y0 = 1 - h, full h to the
/// baseline. Horizontal bars: x0 = 0, w = value fraction.
#[derive(Serialize)]
pub struct Bar {
    pub x0: f64,
    pub y0: f64,
    pub w: f64,
    pub h: f64,
    pub color: Rgb,
}

#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BarDirection {
    Vertical,
    Horizontal,
}

#[derive(Serialize)]
pub struct Source {
    pub mark: crate::spec::Mark,
    pub x_field: String,
    pub y_field: String,
    pub aggregate: Option<Aggregate>,
    /// The resolved x type, set to `Some(Temporal)` ONLY by the temporal xy
    /// path and `None` everywhere else — so `meta()` can tell a temporal x
    /// (ISO domain) from a quantitative one, and every non-temporal snapshot
    /// stays byte-identical (skipped when None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_type: Option<FieldType>,
    /// The `timeUnit` bucketing a temporal bar's x, `Some` ONLY on that path.
    /// `meta()` reports it in the x block (with the canonical bucket keys as the
    /// categories); skipped when None so every other snapshot stays identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_unit: Option<TimeUnit>,
    /// Points-per-series counts etc. needed to reproduce --meta exactly.
    pub series_points: Vec<usize>,
    /// Data provenance (from `Table::provenance`). Drives the conditional
    /// `--meta` data block; always serialized (null when absent) so
    /// `--dump-scene` shows it.
    pub data_source: DataSource,
    pub truncated: Option<bool>,
    pub total_rows: Option<u64>,
}

impl Scene {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scene serialization is infallible")
    }

    /// The --meta payload. Must reproduce the pre-refactor format exactly.
    /// Keys serialize alphabetically (serde_json's default map ordering), so
    /// the order they appear in each `json!` block is irrelevant.
    pub fn meta(&self) -> serde_json::Value {
        let size = json!({ "columns": self.size.columns, "rows": self.size.rows });
        let mut meta = match self.source.mark {
            Mark::Bar => {
                // Orientation is append-only and conditional: a VERTICAL bar
                // reports the pre-existing shape byte-identically (no "direction"
                // key). A HORIZONTAL bar has no x categories (its x is the
                // quantitative value axis) — that's the detector — so it reports
                // x as quantitative-with-domain, y as nominal-with-categories,
                // plus a "direction" key.
                let mut base = if self.x_axis.categories.is_none() {
                    // RAW names, not the truncated tick labels: meta is the
                    // machine-readable surface a caller matches back to rows.
                    let cats = self
                        .y_axis
                        .categories
                        .as_ref()
                        .expect("horizontal bar scenes carry raw y categories");
                    json!({
                        "mark": "bar",
                        "direction": "horizontal",
                        "x": {
                            "field": self.source.x_field,
                            "type": "quantitative",
                            "aggregate": self.source.aggregate,
                            "domain": self.x_axis.domain,
                        },
                        "y": {
                            "field": self.source.y_field,
                            "type": "nominal",
                            "categories": cats,
                        },
                        "dropped_rows": self.dropped_rows,
                        "size": size,
                    })
                } else {
                    // A `timeUnit` bar reports its buckets as the (canonical-key)
                    // categories plus a "timeUnit" tag and a "temporal" type — a
                    // plain bar keeps the byte-identical nominal shape.
                    let mut x = json!({
                        "field": self.source.x_field,
                        "type": "nominal",
                        "categories": self.x_axis.categories,
                    });
                    if let Some(tu) = self.source.time_unit {
                        let x = x.as_object_mut().expect("x meta is an object");
                        x.insert("type".to_string(), json!("temporal"));
                        x.insert("timeUnit".to_string(), json!(tu));
                    }
                    json!({
                    "mark": "bar",
                    "x": x,
                    "y": {
                        "field": self.source.y_field,
                        "aggregate": self.source.aggregate,
                        "domain": self.y_axis.domain,
                    },
                    "dropped_rows": self.dropped_rows,
                    "size": size,
                    })
                };
                // Grouped bars carry a legend; append the xy-shaped series array
                // (name/color/cell-count) from the legend entries zipped with the
                // per-series counts. Plain and tinted bars have no legend and emit
                // byte-identical meta to before.
                if !self.legend.is_empty() {
                    let series: Vec<serde_json::Value> = self
                        .legend
                        .iter()
                        .zip(&self.source.series_points)
                        .map(|(e, count)| {
                            json!({
                                "name": e.name,
                                "color": e.color.hex(),
                                "points": count,
                            })
                        })
                        .collect();
                    base.as_object_mut()
                        .expect("bar meta is an object")
                        .insert("series".to_string(), json!(series));
                }
                base
            }
            Mark::Line | Mark::Point | Mark::Area => {
                // x type/domain: nominal reports its category list; temporal its
                // domain as ISO strings (never raw millis — meta must not lie);
                // quantitative its numeric [min, max]. Series (name/color/count)
                // come from the marks, in first-seen order.
                let (x_type, x_domain) = match &self.x_axis.categories {
                    Some(cats) => ("nominal", json!(cats)),
                    None if self.source.x_type == Some(FieldType::Temporal) => {
                        let d = self
                            .x_axis
                            .domain
                            .expect("temporal x carries a numeric domain");
                        (
                            "temporal",
                            json!([crate::time::format_iso(d[0]), crate::time::format_iso(d[1]),]),
                        )
                    }
                    None => ("quantitative", json!(self.x_axis.domain)),
                };
                let series: Vec<serde_json::Value> = self
                    .marks
                    .iter()
                    .filter_map(|m| {
                        let (sref, count) = match m {
                            SceneMark::Path { series, points }
                            | SceneMark::Points { series, points }
                            | SceneMark::Fill { series, points } => (series, points.len()),
                            SceneMark::Bars { .. } => return None,
                        };
                        Some(json!({
                            "name": sref.name.clone().unwrap_or_default(),
                            "color": sref.color.hex(),
                            "points": count,
                        }))
                    })
                    .collect();
                json!({
                    "mark": self.source.mark,
                    "x": {
                        "field": self.source.x_field,
                        "type": x_type,
                        "domain": x_domain,
                    },
                    "y": {
                        "field": self.source.y_field,
                        "domain": self.y_axis.domain,
                    },
                    "series": series,
                    "dropped_rows": self.dropped_rows,
                    "size": size,
                })
            }
        };
        // The `data` block reports what the caller can't already know from
        // their own bytes: it fires only when the data came from stdin, or the
        // envelope declared truncation info. Inline data is the caller's own
        // bytes, so inline-values/columns charts emit no data block — which
        // keeps the glyph-gallery meta bundles byte-identical.
        let informative = matches!(
            self.source.data_source,
            DataSource::StdinValues | DataSource::StdinColumns
        ) || self.source.truncated.is_some()
            || self.source.total_rows.is_some();
        if informative {
            if let Some(obj) = meta.as_object_mut() {
                obj.insert(
                    "data".to_string(),
                    json!({
                        "source": self.source.data_source,
                        "truncated": self.source.truncated,
                        "total_rows": self.source.total_rows,
                    }),
                );
            }
        }
        meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_stable_json() {
        let scene = Scene {
            size: Size {
                columns: 30,
                rows: 8,
            },
            plot: Rect {
                x: 4,
                y: 1,
                w: 24,
                h: 5,
            },
            chrome: Chrome {
                axis: Rgb(106, 112, 122),
                title: Rgb(222, 226, 232),
            },
            title: Some(Placed {
                text: "Sales".to_string(),
                col: 4,
                row: 0,
            }),
            legend: vec![LegendEntry {
                name: "north".to_string(),
                color: Rgb(0, 128, 255),
                col: 12,
                row: 0,
            }],
            y_axis: YAxis {
                domain: [0.0, 10.0],
                step: 5.0,
                categories: None,
                ticks: vec![
                    YTick {
                        value: 0.0,
                        frac: 0.0,
                        label: "0".to_string(),
                        row: 5,
                    },
                    YTick {
                        value: 10.0,
                        frac: 1.0,
                        label: "10".to_string(),
                        row: 1,
                    },
                ],
            },
            x_axis: XAxis {
                categories: Some(vec!["a".to_string(), "b".to_string()]),
                domain: None,
                tick_cols: vec![],
                labels: vec![Placed {
                    text: "a".to_string(),
                    col: 4,
                    row: 6,
                }],
            },
            marks: vec![SceneMark::Bars {
                bars: vec![
                    Bar {
                        x0: 0.0,
                        y0: 0.4,
                        w: 0.5,
                        h: 0.6,
                        color: Rgb(0, 128, 255),
                    },
                    Bar {
                        x0: 0.5,
                        y0: 0.0,
                        w: 0.5,
                        h: 1.0,
                        color: Rgb(255, 128, 0),
                    },
                ],
                direction: BarDirection::Vertical,
            }],
            dropped_rows: 0,
            source: Source {
                mark: crate::spec::Mark::Bar,
                x_field: "cat".to_string(),
                y_field: "val".to_string(),
                aggregate: None,
                x_type: None,
                time_unit: None,
                series_points: vec![2],
                data_source: DataSource::InlineValues,
                truncated: None,
                total_rows: None,
            },
        };

        insta::assert_snapshot!(scene.to_json(), @r##"
        {
          "size": {
            "columns": 30,
            "rows": 8
          },
          "plot": {
            "x": 4,
            "y": 1,
            "w": 24,
            "h": 5
          },
          "chrome": {
            "axis": "#6a707a",
            "title": "#dee2e8"
          },
          "title": {
            "text": "Sales",
            "col": 4,
            "row": 0
          },
          "legend": [
            {
              "name": "north",
              "color": "#0080ff",
              "col": 12,
              "row": 0
            }
          ],
          "y_axis": {
            "domain": [
              0.0,
              10.0
            ],
            "step": 5.0,
            "ticks": [
              {
                "value": 0.0,
                "frac": 0.0,
                "label": "0",
                "row": 5
              },
              {
                "value": 10.0,
                "frac": 1.0,
                "label": "10",
                "row": 1
              }
            ]
          },
          "x_axis": {
            "categories": [
              "a",
              "b"
            ],
            "domain": null,
            "tick_cols": [],
            "labels": [
              {
                "text": "a",
                "col": 4,
                "row": 6
              }
            ]
          },
          "marks": [
            {
              "Bars": {
                "bars": [
                  {
                    "x0": 0.0,
                    "y0": 0.4,
                    "w": 0.5,
                    "h": 0.6,
                    "color": "#0080ff"
                  },
                  {
                    "x0": 0.5,
                    "y0": 0.0,
                    "w": 0.5,
                    "h": 1.0,
                    "color": "#ff8000"
                  }
                ],
                "direction": "vertical"
              }
            }
          ],
          "dropped_rows": 0,
          "source": {
            "mark": "bar",
            "x_field": "cat",
            "y_field": "val",
            "aggregate": null,
            "series_points": [
              2
            ],
            "data_source": "inline_values",
            "truncated": null,
            "total_rows": null
          }
        }
        "##);
    }
}
