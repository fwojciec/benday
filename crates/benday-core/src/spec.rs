//! The chart specification: a strict subset of Vega-Lite's JSON grammar.
//!
//! Unknown fields are rejected at parse time (`deny_unknown_fields`) so that
//! callers emitting full Vega-Lite get a correctable error instead of a
//! silently wrong chart.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Optional: rows may instead arrive on stdin. `ingest::resolve` enforces
    /// that data is present in exactly one place.
    #[serde(default)]
    pub data: Option<Data>,
    pub mark: Mark,
    pub encoding: Encoding,
    #[serde(default)]
    pub title: Option<String>,
    /// Plot area width in terminal cells (not total output width).
    #[serde(default)]
    pub width: Option<usize>,
    /// Plot area height in terminal cells.
    #[serde(default)]
    pub height: Option<usize>,
}

/// Inline data: tidy row objects, OR columnar `columns` + `rows`. Exactly one
/// form — `ingest::resolve` enforces it (serde can't express either/or here
/// without wrecking error paths).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    /// Inline tidy data: one JSON object per row.
    #[serde(default)]
    pub values: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(default)]
    pub columns: Option<Vec<Column>>,
    #[serde(default)]
    pub rows: Option<Vec<Vec<serde_json::Value>>>,
}

/// Strict twin of `ingest::EnvColumn`: the spec is agent-authored, so unknown
/// keys are rejected here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Bar,
    Line,
    Point,
    Area,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Encoding {
    pub x: Channel,
    pub y: Channel,
    #[serde(default)]
    pub color: Option<Channel>,
    /// Accepted by the grammar only so validation can reject it with a helpful
    /// redirect (grouping is expressed with `color`). Typed as a raw `Value`,
    /// not a `Channel`: Vega-Lite emits several xOffset shapes (`{"field": …}`,
    /// `{"value": …}`, band configs) and a strict `Channel` would bounce them
    /// into serde's generic unknown-field error before validation could help.
    #[serde(default, rename = "xOffset")]
    pub x_offset: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub field: String,
    /// Inferred from the data when omitted.
    #[serde(default, rename = "type")]
    pub ty: Option<FieldType>,
    #[serde(default)]
    pub aggregate: Option<Aggregate>,
    /// Calendar-truncation bucketing for a temporal x on a `bar` mark: each
    /// value is floored to the unit's boundary, keeping the calendar prefix
    /// (`"month"` maps `2026-06-14` to `2026-06`, NOT Vega-Lite's cyclic "all
    /// Junes"). Bar-only, x-only this cycle — every other placement is a
    /// teaching error (see `compile`). Inferred nothing when omitted.
    #[serde(default, rename = "timeUnit")]
    pub time_unit: Option<TimeUnit>,
    /// Histogram binning for a quantitative x on a `bar` mark: `true` (automatic
    /// nice bins), `{"maxbins": N}`, or `{"step": w}`. `false` is accepted and
    /// means absent. Bar-only, x-only this cycle — every other placement is a
    /// teaching error (see `compile`). Value ranges are validated in `preflight`
    /// (not serde) so an out-of-range knob teaches instead of bouncing generically.
    #[serde(default)]
    pub bin: Option<BinValue>,
}

/// `"bin": true` | `{"maxbins": N}` | `{"step": w}`. `false` is accepted and
/// means absent (Vega-Lite emits it; rejecting would be noise). Numbers stay
/// permissive (`f64`) so `preflight` can TEACH — serde must not bounce
/// `maxbins: 0` with a generic type error before validation can explain it.
#[derive(Debug, Clone, Copy)]
pub enum BinValue {
    Flag(bool),
    Config(BinConfig),
}

/// A hand-written `Deserialize` instead of `#[serde(untagged)]`: untagged
/// buffers the value and, on failure, collapses to a generic "data did not
/// match any variant" — swallowing `BinConfig`'s precise unknown-field message.
/// Delegating the map form to `BinConfig` preserves "unknown field `extent`,
/// expected `maxbins` or `step`" (the codebase-wide `deny_unknown_fields`
/// contract) and rejects a bare number/string with a clear `expecting` line —
/// so `{"bin": {"extent": [0,1]}}` and `{"bin": 3}` both fail AT the bin value,
/// never silently matching `Flag`.
impl<'de> Deserialize<'de> for BinValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct BinVisitor;
        impl<'de> serde::de::Visitor<'de> for BinVisitor {
            type Value = BinValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("`true`/`false`, or a bin object with `maxbins` or `step`")
            }
            fn visit_bool<E>(self, v: bool) -> Result<BinValue, E> {
                Ok(BinValue::Flag(v))
            }
            fn visit_map<A>(self, map: A) -> Result<BinValue, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                BinConfig::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(BinValue::Config)
            }
        }
        // JSON is self-describing, so `deserialize_any` dispatches on the actual
        // token (bool → Flag, object → Config; anything else → `expecting`).
        deserializer.deserialize_any(BinVisitor)
    }
}

/// The object form of `bin`. `deny_unknown_fields` rejects stray keys with a
/// named message; both knobs stay permissive `Option<f64>` so `preflight`
/// validates ranges with teaching errors. Neither present (`{}`) means
/// automatic binning, exactly like `true`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinConfig {
    #[serde(default)]
    pub maxbins: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Quantitative,
    Nominal,
    Ordinal,
    /// Continuous time: positioned at true instants on a calendar scale (see
    /// docs/plans/2026-07-05-temporal-family-design.md). An explicit
    /// `"ordinal"` restores evenly-spaced categorical behavior.
    Temporal,
}

/// A calendar-truncation bucket unit for `timeUnit` (temporal bars). Ordered
/// coarse-relevant but semantics are per-variant: truncate a timestamp to this
/// unit's boundary, KEEPING the year (design §spec semantics). Week anchors to
/// Monday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    Hour,
    Minute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Aggregate {
    Sum,
    Mean,
    Median,
    Min,
    Max,
    Count,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal spec wrapping the given `x` channel object — the bin grammar
    /// lives on a channel, so parse checks route through a full `Spec`
    /// (the surface an agent actually posts) to exercise the real path.
    fn parse_x(x: &str) -> Result<Spec, serde_json::Error> {
        serde_json::from_str(&format!(
            r#"{{"data":{{"values":[{{"a":1}}]}},"mark":"bar",
                "encoding":{{"x":{x},"y":{{"field":"a"}}}}}}"#
        ))
    }

    fn bin_of(spec: &Spec) -> Option<BinValue> {
        spec.encoding.x.bin
    }

    #[test]
    fn bin_true_parses_as_flag() {
        let spec = parse_x(r#"{"field":"a","bin":true}"#).expect("bin:true parses");
        assert!(matches!(bin_of(&spec), Some(BinValue::Flag(true))));
    }

    #[test]
    fn bin_false_parses_as_flag() {
        // `false` is accepted (Vega-Lite emits it) and means absent — the
        // channel-placement checks treat it as if `bin` were omitted.
        let spec = parse_x(r#"{"field":"a","bin":false}"#).expect("bin:false parses");
        assert!(matches!(bin_of(&spec), Some(BinValue::Flag(false))));
    }

    #[test]
    fn bin_absent_is_none() {
        let spec = parse_x(r#"{"field":"a"}"#).expect("no bin parses");
        assert!(bin_of(&spec).is_none());
    }

    #[test]
    fn bin_maxbins_parses_as_config() {
        let spec = parse_x(r#"{"field":"a","bin":{"maxbins":15}}"#).expect("maxbins parses");
        assert!(matches!(
            bin_of(&spec),
            Some(BinValue::Config(BinConfig {
                maxbins: Some(m),
                step: None
            })) if m == 15.0
        ));
    }

    #[test]
    fn bin_step_parses_as_config() {
        let spec = parse_x(r#"{"field":"a","bin":{"step":10}}"#).expect("step parses");
        assert!(matches!(
            bin_of(&spec),
            Some(BinValue::Config(BinConfig {
                maxbins: None,
                step: Some(s)
            })) if s == 10.0
        ));
    }

    #[test]
    fn bin_empty_config_parses() {
        // `{}` is the automatic-binning shape (bin:true parity), NOT an error.
        let spec = parse_x(r#"{"field":"a","bin":{}}"#).expect("empty config parses");
        assert!(matches!(
            bin_of(&spec),
            Some(BinValue::Config(BinConfig {
                maxbins: None,
                step: None
            }))
        ));
    }

    // Permissive numbers: serde MUST accept out-of-range knobs so `preflight`
    // can teach instead of serde bouncing the spec with a generic type error.
    #[test]
    fn bin_maxbins_zero_parses() {
        parse_x(r#"{"field":"a","bin":{"maxbins":0}}"#)
            .expect("maxbins:0 parses (preflight teaches)");
    }

    #[test]
    fn bin_maxbins_fractional_parses() {
        parse_x(r#"{"field":"a","bin":{"maxbins":2.5}}"#)
            .expect("maxbins:2.5 parses (preflight teaches)");
    }

    #[test]
    fn bin_step_negative_parses() {
        parse_x(r#"{"field":"a","bin":{"step":-1}}"#).expect("step:-1 parses (preflight teaches)");
    }

    #[test]
    fn bin_step_zero_parses() {
        parse_x(r#"{"field":"a","bin":{"step":0}}"#).expect("step:0 parses (preflight teaches)");
    }

    // `deny_unknown_fields` on BinConfig: an unknown key inside the object must
    // ERROR — never silently fall through to `Flag` — and NAME the stray field
    // (the manual `Deserialize` preserves BinConfig's precise message).
    #[test]
    fn bin_unknown_field_errors() {
        let err = parse_x(r#"{"field":"a","bin":{"extent":[0,1]}}"#)
            .expect_err("unknown bin field must error, not match Flag");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown field `extent`") && msg.contains("maxbins"),
            "extent error must name the stray field and the valid knobs: {msg}"
        );
    }

    // A bare number is neither a bool nor the object form: it must ERROR at the
    // bin value with the `expecting` line, never silently match.
    #[test]
    fn bin_bare_number_errors() {
        let err =
            parse_x(r#"{"field":"a","bin":3}"#).expect_err("bare-number bin must error, not match");
        let msg = err.to_string();
        assert!(
            msg.contains("maxbins") || msg.contains("bin object"),
            "bare-number error must point at the bin shape: {msg}"
        );
    }
}
