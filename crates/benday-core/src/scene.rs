//! The Scene: benday's intermediate representation. `compile()` resolves a
//! spec against its data into a Scene — every data- and layout-dependent
//! decision made, geometry normalized to the plot rect — and `rasterize()`
//! turns a Scene into glyphs. The serialized form is the golden-corpus
//! snapshot and the `--dump-scene` output; it is explicitly unstable.

use serde::Serialize;

use crate::raster::Rgb;
use crate::spec::Aggregate;

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
    /// Deduped, in draw order. `row` is buffer-absolute.
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

#[derive(Serialize)]
pub struct Bar {
    /// Left edge and width as fractions of plot width (exact multiples of 1/plot_w).
    pub x0: f64,
    pub w: f64,
    /// Height as a fraction of plot height (y.norm of the aggregated value).
    pub h: f64,
    pub color: Rgb,
}

#[derive(Serialize)]
pub struct Source {
    pub mark: crate::spec::Mark,
    pub x_field: String,
    pub y_field: String,
    pub aggregate: Option<Aggregate>,
    /// Points-per-series counts etc. needed to reproduce --meta exactly.
    pub series_points: Vec<usize>,
}

impl Scene {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scene serialization is infallible")
    }

    /// The --meta payload. Must reproduce the pre-refactor format exactly.
    // Implemented in Task 4/5, once compile()/rasterize() populate the Scene
    // with the per-mark facts --meta reports. `clippy::todo` is allow-by-default
    // and the `clippy::panic` ratchet does not cover `todo!`, so no allow needed.
    pub fn meta(&self) -> serde_json::Value {
        todo!("implemented in Task 4/5 per mark type")
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
                        w: 0.5,
                        h: 0.6,
                        color: Rgb(0, 128, 255),
                    },
                    Bar {
                        x0: 0.5,
                        w: 0.5,
                        h: 1.0,
                        color: Rgb(255, 128, 0),
                    },
                ],
            }],
            dropped_rows: 0,
            source: Source {
                mark: crate::spec::Mark::Bar,
                x_field: "cat".to_string(),
                y_field: "val".to_string(),
                aggregate: None,
                series_points: vec![2],
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
                    "w": 0.5,
                    "h": 0.6,
                    "color": "#0080ff"
                  },
                  {
                    "x0": 0.5,
                    "w": 0.5,
                    "h": 1.0,
                    "color": "#ff8000"
                  }
                ]
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
            ]
          }
        }
        "##);
    }
}
