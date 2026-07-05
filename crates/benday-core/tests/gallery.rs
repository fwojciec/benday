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
        (
            "legend_wrap_5_series",
            r#"{"data":{"values":[
                {"x":1,"y":2,"s":"hot-full-table-scan"},{"x":2,"y":3,"s":"hot-full-table-scan"},
                {"x":1,"y":3,"s":"hot-window-projection"},{"x":2,"y":4,"s":"hot-window-projection"},
                {"x":1,"y":4,"s":"wide-50-columns"},{"x":2,"y":5,"s":"wide-50-columns"},
                {"x":1,"y":5,"s":"sparse-200-columns"},{"x":2,"y":6,"s":"sparse-200-columns"},
                {"x":1,"y":6,"s":"ultra-sparse-800"},{"x":2,"y":7,"s":"ultra-sparse-800"}]},
              "mark":"line","title":"legend wraps, never clips",
              "encoding":{"x":{"field":"x"},"y":{"field":"y"},"color":{"field":"s"}}}"#,
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

/// The bar family beyond plain vertical bars: content-sized horizontal
/// rankings, grouped vertical bars (color = a third field), grouped
/// horizontal, and a colored grouped variant so the ANSI path for legend +
/// series colors is characterized. Horizontal charts size their height to the
/// category count, so those cases pass `height: None` — a fixed height would
/// (correctly) be rejected as over-ceiling for content-sized rankings.
#[test]
fn bar_family_gallery() {
    // Content-sized horizontal ranking: 8 facilities, some names past the
    // 24-cell gutter so the `…` truncation shows. No height — the row count
    // is derived from the data.
    let ranking = parse(
        r#"{"data":{"values":[
            {"facility":"St. Mary's Regional Medical Center","volume":1284},
            {"facility":"Cedar Grove Community Hospital","volume":1102},
            {"facility":"Northlake Cardiovascular Institute","volume":968},
            {"facility":"Riverside General Hospital","volume":947},
            {"facility":"Lakeshore Memorial","volume":806},
            {"facility":"Pinecrest Health System","volume":651},
            {"facility":"Fairview Medical","volume":540},
            {"facility":"Oak Ridge Clinic","volume":388}]},
          "mark":"bar","title":"referral volume by facility",
          "encoding":{"x":{"field":"volume"},"y":{"field":"facility"}}}"#,
    );
    snap(
        "ranking_horizontal",
        &ranking,
        &RenderOptions {
            width: Some(60),
            height: None,
            ..opts(60, 10)
        },
    );

    // Grouped vertical bars: color names a third field (referral direction),
    // so each quarter gets a paired in/out cluster.
    let grouped = parse(
        r#"{"data":{"values":[
            {"q":"Q1","dir":"in","n":40},{"q":"Q1","dir":"out","n":25},
            {"q":"Q2","dir":"in","n":55},{"q":"Q2","dir":"out","n":30},
            {"q":"Q3","dir":"in","n":62},{"q":"Q3","dir":"out","n":44},
            {"q":"Q4","dir":"in","n":71},{"q":"Q4","dir":"out","n":38}]},
          "mark":"bar","title":"referrals in vs out by quarter",
          "encoding":{"x":{"field":"q"},"y":{"field":"n"},"color":{"field":"dir"}}}"#,
    );
    snap("grouped_bars_referrals", &grouped, &opts(60, 10));

    // Colored variant of the same grouped chart: characterizes the ANSI path
    // for the legend and per-series bar colors.
    snap(
        "grouped_bars_ansi",
        &grouped,
        &RenderOptions {
            color: true,
            ..opts(60, 10)
        },
    );

    // Small grouped horizontal: 3 facilities × 2 series, content-sized height.
    let grouped_h = parse(
        r#"{"data":{"values":[
            {"facility":"St. Mary's Regional","dir":"inbound","n":420},
            {"facility":"St. Mary's Regional","dir":"outbound","n":260},
            {"facility":"Cedar Grove Community","dir":"inbound","n":310},
            {"facility":"Cedar Grove Community","dir":"outbound","n":190},
            {"facility":"Riverside General","dir":"inbound","n":275},
            {"facility":"Riverside General","dir":"outbound","n":140}]},
          "mark":"bar","title":"inbound vs outbound by facility",
          "encoding":{"x":{"field":"n"},"y":{"field":"facility"},"color":{"field":"dir"}}}"#,
    );
    snap(
        "grouped_horizontal_small",
        &grouped_h,
        &RenderOptions {
            width: Some(50),
            height: None,
            ..opts(50, 10)
        },
    );
}

/// The temporal family: continuous-time line marks and timeUnit-bucketed bars.
/// Every case pins an explicit size chosen so the calendar-tick ladder selects
/// the NAMED step at that width — or, for the narrow fallback case, exhausts
/// the ladder to the two-endpoint fallback. The label idiom is the thing under
/// test, so the width is load-bearing, not incidental. Sizes verified by a real-terminal
/// render before pinning: a narrower width would coarsen the rung and change the
/// labels. Line marks place values at true positions in time; timeUnit bars
/// truncate to canonical ISO buckets, then densify so empty calendar cells keep
/// their tick and (for `count`) a zero bar.
#[test]
fn temporal_family_gallery() {
    // Daily line over ~3.6 weeks: the week rung is selected, so ticks land on
    // Mondays (Jun 1, 8, 15, 22, 29) with the year context only at the first
    // tick. The domain expands outward to the enclosing week boundaries.
    let daily = parse(
        r#"{"data":{"columns":[{"name":"day","type":"DATE"},{"name":"sessions","type":"INT64"}],
            "rows":[
              ["2026-06-01",120],["2026-06-02",138],["2026-06-03",145],["2026-06-04",132],
              ["2026-06-05",150],["2026-06-06",168],["2026-06-07",142],["2026-06-08",159],
              ["2026-06-09",171],["2026-06-10",165],["2026-06-11",180],["2026-06-12",176],
              ["2026-06-13",190],["2026-06-14",185],["2026-06-15",172],["2026-06-16",168],
              ["2026-06-17",159],["2026-06-18",182],["2026-06-19",195],["2026-06-20",201],
              ["2026-06-21",188],["2026-06-22",176],["2026-06-23",192],["2026-06-24",205],
              ["2026-06-25",198],["2026-06-26",210]]},
          "mark":"line","encoding":{"x":{"field":"day"},"y":{"field":"sessions"}}}"#,
    );
    snap("temporal_line_daily_week_ticks", &daily, &opts(72, 8));

    // The same dataset width-starved: a June-only domain gives month, quarter,
    // and year rungs only two ticks each (under MIN_TICKS), so once the week
    // rung collides there is no rung left — the first-and-last fallback fires.
    // Structurally distinct from every rung case: exactly two FULL-context
    // labels at the TRUE data extremes (ticks at the frame edges), domain
    // tight to min/max with no boundary expansion. 16 is the narrowest width
    // where both labels place — the first clamps left over the gutter, the
    // second clamps right to the row end, one column apart; at 15 the greedy
    // placement drops the second label.
    snap("temporal_line_fallback_narrow", &daily, &opts(16, 8));

    // 16 monthly points, Jan '25 → Apr '26: the quarter rung is selected, and the
    // year context reappears at the Q1 '26 rollover — Q1 '25 · Q2 · Q3 · Q4 ·
    // Q1 '26 · Q2. This is the analytics-cube period at its native step.
    let quarterly = parse(
        r#"{"data":{"columns":[{"name":"month","type":"DATE"},{"name":"mrr","type":"INT64"}],
            "rows":[
              ["2025-01-01",240],["2025-02-01",255],["2025-03-01",270],["2025-04-01",262],
              ["2025-05-01",288],["2025-06-01",295],["2025-07-01",310],["2025-08-01",305],
              ["2025-09-01",330],["2025-10-01",348],["2025-11-01",352],["2025-12-01",360],
              ["2026-01-01",372],["2026-02-01",368],["2026-03-01",390],["2026-04-01",405]]},
          "mark":"line","encoding":{"x":{"field":"month"},"y":{"field":"mrr"}}}"#,
    );
    snap("temporal_line_quarterly_rollover", &quarterly, &opts(72, 8));

    // Gappy readings: three clusters with true calendar gaps between them. On an
    // ordinal axis the gaps would vanish; the temporal scale renders them as
    // proportional blank space — positional truth is the point of this case.
    let gappy = parse(
        r#"{"data":{"columns":[{"name":"t","type":"DATE"},{"name":"latency_ms","type":"INT64"}],
            "rows":[
              ["2026-03-02",410],["2026-03-03",455],["2026-03-05",430],
              ["2026-03-18",680],["2026-03-19",720],["2026-03-20",695],
              ["2026-04-06",540],["2026-04-07",560]]},
          "mark":"line","encoding":{"x":{"field":"t"},"y":{"field":"latency_ms"}}}"#,
    );
    snap("temporal_line_gappy", &gappy, &opts(72, 8));

    // Raw log timestamps, INLINE and UNDECLARED (the promoted-string path): every
    // value parses as ISO, so the column is inferred temporal end to end. timeUnit
    // hour + count with no separate value field is the events-per-hour histogram.
    // Hours 11–13 have no events; densify inserts them as zero bars — a quiet-hours
    // run that keeps its ticks. This is the raw-gcloud-logs story in one spec.
    let hourly = parse(
        r#"{"data":{"values":[
              {"ts":"2026-06-14T08:03:11"},{"ts":"2026-06-14T08:19:44"},
              {"ts":"2026-06-14T08:37:02"},{"ts":"2026-06-14T08:51:59"},
              {"ts":"2026-06-14T09:02:00"},{"ts":"2026-06-14T09:08:31"},
              {"ts":"2026-06-14T09:22:10"},{"ts":"2026-06-14T09:33:45"},
              {"ts":"2026-06-14T09:47:20"},{"ts":"2026-06-14T09:58:04"},
              {"ts":"2026-06-14T10:05:12"},{"ts":"2026-06-14T10:28:39"},
              {"ts":"2026-06-14T10:55:01"},{"ts":"2026-06-14T14:11:07"},
              {"ts":"2026-06-14T14:19:52"},{"ts":"2026-06-14T14:33:20"},
              {"ts":"2026-06-14T14:48:09"},{"ts":"2026-06-14T14:59:41"},
              {"ts":"2026-06-14T15:14:33"},{"ts":"2026-06-14T15:42:18"}]},
          "mark":"bar",
          "encoding":{"x":{"field":"ts","timeUnit":"hour"},"y":{"field":"ts","aggregate":"count"}}}"#,
    );
    snap("temporal_bar_hourly_count_quiet", &hourly, &opts(72, 8));

    // Daily revenue rows bucketed and summed by month, Sep '25 → Apr '26: the
    // month rung labels every bucket, year context at the first tick and at the
    // Jan '26 rollover. SQL would date_trunc here; when SQL is absent, timeUnit
    // does the bucketing and benday still owns the axis geometry.
    let month = parse(
        r#"{"data":{"columns":[{"name":"d","type":"DATE"},{"name":"revenue","type":"INT64"}],
            "rows":[
              ["2025-09-03",120],["2025-09-08",90],["2025-09-13",140],
              ["2025-10-03",160],["2025-10-08",110],
              ["2025-11-03",130],["2025-11-08",150],["2025-11-13",80],
              ["2025-12-03",200],["2025-12-08",190],
              ["2026-01-03",90],["2026-01-08",70],["2026-01-13",60],
              ["2026-02-03",210],["2026-02-08",180],
              ["2026-03-03",240],["2026-03-08",60],["2026-03-13",90],
              ["2026-04-03",300],["2026-04-08",120]]},
          "mark":"bar",
          "encoding":{"x":{"field":"d","timeUnit":"month"},"y":{"field":"revenue","aggregate":"sum"}}}"#,
    );
    snap("temporal_bar_month_sum", &month, &opts(72, 8));
}
