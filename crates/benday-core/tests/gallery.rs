//! Characterization snapshots: the exact rendered output of every example and
//! a set of adversarial specs. These are the referee for the Scene IR
//! refactor — a diff here without an intended visual change means a bug.

use std::fs;
use std::path::Path;

use benday_core::{render, spec::Spec, theme, BarStyle, Marker, RenderOptions};

fn opts(width: usize, height: usize) -> RenderOptions {
    RenderOptions {
        width: Some(width),
        height: Some(height),
        marker: Marker::Braille,
        bar_style: BarStyle::Dots,
        theme: theme::by_name("benday").unwrap(),
        color: false,
    }
}

fn snap(name: &str, spec: &Spec, o: &RenderOptions) {
    let out = render(spec, None, o).unwrap();
    let bundle = format!(
        "{}\n--- meta ---\n{}",
        out.text,
        serde_json::to_string_pretty(&out.meta).unwrap()
    );
    insta::assert_snapshot!(name.to_string(), bundle);
}

fn parse(json: &str) -> Spec {
    serde_json::from_str(json).unwrap()
}

#[test]
fn examples_gallery() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no example specs found");
    for path in &paths {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let spec: Spec = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        for (w, h) in [(60, 10), (30, 6)] {
            snap(&format!("{stem}_{w}x{h}"), &spec, &opts(w, h));
        }
    }
}

#[test]
fn adversarial_gallery() {
    let cases: &[(&str, &str)] = &[
        (
            "long_labels",
            r#"{"data":{"values":[
                {"c":"extraordinarily long category name","v":4},
                {"c":"another very long one indeed","v":9},
                {"c":"short","v":2}]},
              "mark":"bar","encoding":{"x":{"field":"c"},"y":{"field":"v"}}}"#,
        ),
        (
            "single_point_line",
            r#"{"data":{"values":[{"x":5,"y":3}]},
              "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        ),
        (
            "count_aggregate",
            r#"{"data":{"values":[{"k":"a"},{"k":"a"},{"k":"b"},{"k":"a"},{"k":"c"}]},
              "mark":"bar",
              "encoding":{"x":{"field":"k"},"y":{"field":"k","aggregate":"count"}}}"#,
        ),
        (
            "negative_line",
            r#"{"data":{"values":[{"x":0,"y":-20},{"x":1,"y":15},{"x":2,"y":-5},{"x":3,"y":30}]},
              "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        ),
        (
            "numeric_strings",
            r#"{"data":{"values":[{"m":"a","v":"12"},{"m":"b","v":"7.5"}]},
              "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        ),
        (
            "small_height_ticks_h7",
            r#"{"data":{"values":[{"m":"a","v":160},{"m":"b","v":40}]},
              "mark":"bar","height":7,
              "encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
        ),
    ];
    for (name, json) in cases {
        let spec = parse(json);
        let (w, h) = (60, 10);
        // small_height_ticks_h7 relies on spec.height; don't override it.
        let o = if *name == "small_height_ticks_h7" {
            RenderOptions {
                width: Some(w),
                height: None,
                ..opts(w, h)
            }
        } else {
            opts(w, h)
        };
        snap(name, &spec, &o);
    }
}

#[test]
fn style_variants() {
    let bar = parse(
        r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7},{"m":"mar","v":5}]},
          "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
    );
    snap(
        "bar_blocks",
        &bar,
        &RenderOptions {
            bar_style: BarStyle::Blocks,
            ..opts(60, 10)
        },
    );
    let line = parse(
        r#"{"data":{"values":[{"x":0,"y":1},{"x":1,"y":4},{"x":2,"y":2},{"x":3,"y":6}]},
          "mark":"line","encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
    );
    snap(
        "line_octant",
        &line,
        &RenderOptions {
            marker: Marker::Octant,
            ..opts(60, 10)
        },
    );
}

/// Color is ON by default for CLI callers, so the ANSI path needs its own
/// characterization — the no-color gallery cannot see a regression in theme
/// colors for title/legend/axis chrome or per-mark colors. Escape codes make
/// these snapshots ugly to read; their job is diffing, not reading.
#[test]
fn colored_variants() {
    let bar = parse(
        r#"{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7},{"m":"mar","v":5}]},
          "mark":"bar","title":"colored bar",
          "encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#,
    );
    snap(
        "bar_ansi",
        &bar,
        &RenderOptions {
            color: true,
            ..opts(60, 10)
        },
    );
    let lines = parse(
        r#"{"data":{"values":[
            {"m":"a","v":1,"r":"west"},{"m":"b","v":4,"r":"west"},
            {"m":"a","v":2,"r":"east"},{"m":"b","v":3,"r":"east"}]},
          "mark":"line","title":"colored lines",
          "encoding":{"x":{"field":"m"},"y":{"field":"v"},"color":{"field":"r"}}}"#,
    );
    snap(
        "multi_series_ansi",
        &lines,
        &RenderOptions {
            color: true,
            ..opts(60, 10)
        },
    );
}
