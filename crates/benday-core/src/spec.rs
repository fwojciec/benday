//! The chart specification: a strict subset of Vega-Lite's JSON grammar.
//!
//! Unknown fields are rejected at parse time (`deny_unknown_fields`) so that
//! callers emitting full Vega-Lite get a correctable error instead of a
//! silently wrong chart.

use serde::{Deserialize, Serialize};

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
