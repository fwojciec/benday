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

/// Infer a field's type from its present, non-null values. All coerce to a
/// number → Quantitative; else all are temporal strings (`time::parse_temporal`
/// accepts them) → Temporal; else Nominal. Promotion is all-or-nothing: one
/// value that is neither drops the whole column to Nominal — a column mixing
/// dates and numbers is Nominal, no partial parsing (temporal-family design).
pub fn infer_type(rows: &[Row], field: &str) -> FieldType {
    let mut saw_value = false;
    let mut all_numeric = true;
    let mut all_temporal = true;
    for row in rows {
        if let Some(v) = row.get(field) {
            if v.is_null() {
                continue;
            }
            saw_value = true;
            // Each predicate runs only while its flag still stands — a
            // falsified flag stops paying for its check.
            if all_numeric && num(v).is_none() {
                all_numeric = false;
            }
            if all_temporal
                && !matches!(v, Value::String(s) if crate::time::parse_temporal(s).is_some())
            {
                all_temporal = false;
            }
            if !all_numeric && !all_temporal {
                break; // Nominal is already decided.
            }
        }
    }
    if !saw_value {
        FieldType::Nominal
    } else if all_numeric {
        FieldType::Quantitative
    } else if all_temporal {
        FieldType::Temporal
    } else {
        FieldType::Nominal
    }
}
