//! The chart specification: a strict subset of Vega-Lite's JSON grammar.
//!
//! Unknown fields are rejected at parse time (`deny_unknown_fields`) so that
//! callers emitting full Vega-Lite get a correctable error instead of a
//! silently wrong chart.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub data: Data,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    /// Inline tidy data: one JSON object per row.
    pub values: Vec<serde_json::Map<String, serde_json::Value>>,
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
