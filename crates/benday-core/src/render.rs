//! The public render entry point: compile the spec into a `Scene`, then
//! rasterize that Scene into ANSI text plus the `--meta` payload. All the real
//! work lives in `compile` and `raster`; this module only adapts options and
//! owns the public option/output types.

use crate::compile;
use crate::error::Error;
use crate::ingest::{self, DataDoc};
use crate::raster::{self, Marker};
use crate::spec::Spec;
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

pub fn render(spec: &Spec, data: Option<DataDoc>, opts: &RenderOptions) -> Result<Rendered, Error> {
    // Resolve the spec's inline data and/or the piped data document into a
    // Table, then compile to a Scene (which owns preflight validation) and
    // rasterize. No per-mark branching here.
    let table = ingest::resolve(spec, data)?;
    let copts = compile::CompileOptions {
        width: opts.width,
        height: opts.height,
        theme: opts.theme.clone(),
    };
    let scene = compile::compile(spec, &table, &copts)?;
    let ropts = raster::RasterOptions {
        marker: opts.marker,
        bar_style: opts.bar_style,
        color: opts.color,
    };
    Ok(raster::rasterize(&scene, &ropts))
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
        let out = render(&s, None, &opts()).unwrap();
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
            None,
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
        let out = render(&s, None, &opts()).unwrap();
        assert!(out.text.chars().any(is_braille));
    }

    #[test]
    fn missing_field_is_actionable() {
        let s = spec(
            r#"{"data":{"values":[{"month":"jan","sales":3}]},
                "mark":"bar","encoding":{"x":{"field":"month"},"y":{"field":"revenue"}}}"#,
        );
        let err = render(&s, None, &opts()).unwrap_err();
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
        let err = render(&s, None, &opts()).unwrap_err();
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
        let err = render(&s, None, &opts()).unwrap_err();
        assert_eq!(err.kind(), "spec");
        assert!(err.to_string().contains("color"));
    }
}
