//! Row access, coercion, and type inference over inline JSON data.

use serde_json::{Map, Value};

use crate::error::Error;
use crate::spec::FieldType;

pub type Row = Map<String, Value>;

/// Coerce a JSON value to a number. Numeric strings count: query engines
/// frequently serialize decimals as strings.
pub fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Categorical representation of a JSON value.
pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

pub fn check_field(rows: &[Row], field: &str) -> Result<(), Error> {
    if rows.iter().any(|r| r.contains_key(field)) {
        return Ok(());
    }
    let mut fields: Vec<&str> = rows
        .iter()
        .flat_map(|r| r.keys().map(String::as_str))
        .collect();
    fields.sort_unstable();
    fields.dedup();
    Err(Error::Data(format!(
        "field \"{field}\" not found in data; available fields: {}",
        fields.join(", ")
    )))
}

/// Quantitative iff every present, non-null value coerces to a number.
pub fn infer_type(rows: &[Row], field: &str) -> FieldType {
    let mut saw_value = false;
    for row in rows {
        if let Some(v) = row.get(field) {
            if v.is_null() {
                continue;
            }
            saw_value = true;
            if num(v).is_none() {
                return FieldType::Nominal;
            }
        }
    }
    if saw_value {
        FieldType::Quantitative
    } else {
        FieldType::Nominal
    }
}
