//! Data ingestion: resolve the spec's inline data and/or a piped data
//! document into a normalized `Table`. Pure — the CLI does the I/O.
//!
//! Strictness boundary: the SPEC is agent-authored intent, so its data object
//! is strict (`deny_unknown_fields`, over in `spec.rs`). The stdin document is
//! producer-shaped payload (e.g. an MCP `structuredContent` envelope), so it
//! is tolerant: known fields are used, unknown fields (query provenance etc.)
//! are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Error;
use crate::spec::{FieldType, Spec};

pub type Row = Map<String, Value>;

/// A data document piped to stdin: a columnar envelope or a bare row array.
/// Tolerant by design — no `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DataDoc {
    Envelope {
        columns: Vec<EnvColumn>,
        rows: Vec<Vec<Value>>,
        #[serde(default)]
        truncated: Option<bool>,
        #[serde(default)]
        total_rows: Option<u64>,
    },
    Rows(Vec<Row>),
}

/// Envelope column: tolerant twin of `spec::Column` (producers may add keys).
#[derive(Debug, Deserialize)]
pub struct EnvColumn {
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    InlineValues,
    InlineColumns,
    StdinValues,
    StdinColumns,
}

#[derive(Debug, Serialize)]
pub struct DataProvenance {
    pub source: DataSource,
    pub truncated: Option<bool>,
    pub total_rows: Option<u64>,
}

/// Normalized data ready for the compiler: row-major rows (as the compiler
/// has always consumed), declared column types, and where it all came from.
#[derive(Debug)]
pub struct Table {
    pub rows: Vec<Row>,
    pub declared: HashMap<String, FieldType>,
    pub provenance: DataProvenance,
}

/// Parse a stdin data document. Wraps serde's unhelpful untagged-enum error
/// with the two accepted shapes.
pub fn parse_data_doc(s: &str) -> Result<DataDoc, Error> {
    serde_json::from_str(s).map_err(|e| {
        Error::Data(format!(
            "cannot parse stdin as a data document; expected \
             {{\"columns\":[{{\"name\",\"type\"?}}...],\"rows\":[[...]...]}} or a JSON array \
             of row objects: {e}"
        ))
    })
}

/// Map a declared column type (BigQuery + common SQL spellings, case-
/// insensitive) to a field type. Unknown names fall back to nominal — NOT an
/// error: producers grow types, and nominal is safe-wrong-in-the-obvious-way.
/// DATE/TIMESTAMP map to ordinal THIS CYCLE (ISO strings sort lexically =
/// chronologically); they become temporal in the next cycle.
pub fn declared_field_type(t: &str) -> FieldType {
    match t.to_ascii_uppercase().as_str() {
        "INT64" | "INTEGER" | "INT" | "SMALLINT" | "BIGINT" | "FLOAT64" | "FLOAT" | "DOUBLE"
        | "NUMERIC" | "BIGNUMERIC" | "DECIMAL" | "REAL" => FieldType::Quantitative,
        "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" => FieldType::Ordinal,
        _ => FieldType::Nominal,
    }
}

/// Resolve spec + optional stdin document into a Table. Owns ALL precedence
/// and data-shape errors so the corpus can pin them.
pub fn resolve(spec: &Spec, stdin: Option<DataDoc>) -> Result<Table, Error> {
    // The spec-side inline data. Today `spec.data` is required-`values`, so
    // this is always `Some`; Task 3 makes it an `Option` and adds the inline
    // columnar form. Structured as a match now so Task 3 only touches the
    // spec-side pattern, never the stdin arms.
    let inline = Some(&spec.data);

    match (inline, stdin) {
        (Some(_), Some(_)) => Err(Error::Spec(
            "data provided twice: the spec has inline `data` and a data document \
             arrived on stdin; remove one"
                .into(),
        )),
        (Some(data), None) => {
            // Inline `values`: row-major already, no declared types.
            finish(
                data.values.clone(),
                HashMap::new(),
                DataSource::InlineValues,
                None,
                None,
            )
        }
        (None, Some(doc)) => resolve_stdin(doc),
        (None, None) => Err(Error::Spec(
            "no data: the spec has no `data` and nothing arrived on stdin; add \
             data.values or data.columns+rows to the spec, or pipe a data document"
                .into(),
        )),
    }
}

/// Resolve a stdin data document (envelope or bare rows) into a Table.
fn resolve_stdin(doc: DataDoc) -> Result<Table, Error> {
    match doc {
        DataDoc::Envelope {
            columns,
            rows,
            truncated,
            total_rows,
        } => {
            let (rows, declared) = columnar_to_rows(&columns, &rows)?;
            finish(
                rows,
                declared,
                DataSource::StdinColumns,
                truncated,
                total_rows,
            )
        }
        DataDoc::Rows(rows) => finish(rows, HashMap::new(), DataSource::StdinValues, None, None),
    }
}

/// Zip a columnar envelope into row-major objects, keyed in column order, and
/// collect declared column types. Shared shape for inline and stdin columnar.
fn columnar_to_rows(
    columns: &[EnvColumn],
    rows: &[Vec<Value>],
) -> Result<(Vec<Row>, HashMap<String, FieldType>), Error> {
    let mut declared = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for col in columns {
        if !seen.insert(col.name.as_str()) {
            return Err(Error::Data(format!(
                "duplicate column name \"{}\"",
                col.name
            )));
        }
        if let Some(ty) = &col.ty {
            declared.insert(col.name.clone(), declared_field_type(ty));
        }
    }

    let want = columns.len();
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        if row.len() != want {
            return Err(Error::Data(format!(
                "row {i} has {} values but {want} columns are declared",
                row.len()
            )));
        }
        let mut obj = Row::new();
        for (col, val) in columns.iter().zip(row) {
            obj.insert(col.name.clone(), val.clone());
        }
        out.push(obj);
    }
    Ok((out, declared))
}

/// Apply the empty-rows rule (any form) and assemble the Table.
fn finish(
    rows: Vec<Row>,
    declared: HashMap<String, FieldType>,
    source: DataSource,
    truncated: Option<bool>,
    total_rows: Option<u64>,
) -> Result<Table, Error> {
    if rows.is_empty() {
        return Err(Error::Data(
            "data has no rows; provide at least one row".into(),
        ));
    }
    Ok(Table {
        rows,
        declared,
        provenance: DataProvenance {
            source,
            truncated,
            total_rows,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec_with_values() -> Spec {
        serde_json::from_str(
            r#"{"data":{"values":[{"x":1}]},"mark":"bar",
               "encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        )
        .expect("fixture spec parses")
    }

    fn spec_with_empty_values() -> Spec {
        serde_json::from_str(
            r#"{"data":{"values":[]},"mark":"bar",
               "encoding":{"x":{"field":"x"},"y":{"field":"y"}}}"#,
        )
        .expect("fixture spec parses")
    }

    #[test]
    fn parses_bare_row_array() {
        let doc = parse_data_doc(r#"[{"a": 1}, {"a": 2}]"#).expect("bare array parses");
        match doc {
            DataDoc::Rows(rows) => assert_eq!(rows.len(), 2),
            other => panic!("expected Rows, got {other:?}"),
        }
    }

    #[test]
    fn envelope_tolerates_unknown_keys() {
        // The producer emits a `query` provenance block benday ignores.
        let doc = parse_data_doc(
            r#"{"columns":[{"name":"day","type":"STRING"},{"name":"n","type":"INT64"}],
                "rows":[["mon",32]],
                "query":{"job_id":"abc","note":"ignored"}}"#,
        )
        .expect("envelope with extra keys parses");
        match doc {
            DataDoc::Envelope { columns, rows, .. } => {
                assert_eq!(columns.len(), 2);
                assert_eq!(rows.len(), 1);
            }
            other => panic!("expected Envelope, got {other:?}"),
        }
    }

    #[test]
    fn envelope_truncated_and_total_rows_reach_provenance() {
        let doc = parse_data_doc(
            r#"{"columns":[{"name":"n"}],"rows":[[1]],"truncated":true,"total_rows":123}"#,
        )
        .expect("envelope parses");
        let table = resolve_stdin(doc).expect("resolves");
        assert_eq!(table.provenance.source, DataSource::StdinColumns);
        assert_eq!(table.provenance.truncated, Some(true));
        assert_eq!(table.provenance.total_rows, Some(123));
    }

    #[test]
    fn bare_rows_provenance_has_no_envelope_fields() {
        let doc = parse_data_doc(r#"[{"a":1}]"#).expect("parses");
        let table = resolve_stdin(doc).expect("resolves");
        assert_eq!(table.provenance.source, DataSource::StdinValues);
        assert_eq!(table.provenance.truncated, None);
        assert_eq!(table.provenance.total_rows, None);
        assert!(table.declared.is_empty());
    }

    #[test]
    fn columnar_zips_rows_in_column_order() {
        let doc = parse_data_doc(
            r#"{"columns":[{"name":"day","type":"STRING"},{"name":"n","type":"INT64"}],
                "rows":[["mon",32],["tue",78]]}"#,
        )
        .expect("parses");
        let table = resolve_stdin(doc).expect("resolves");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].get("day"), Some(&json!("mon")));
        assert_eq!(table.rows[0].get("n"), Some(&json!(32)));
        assert_eq!(table.rows[1].get("day"), Some(&json!("tue")));
        assert_eq!(table.rows[1].get("n"), Some(&json!(78)));
        // keys land in declared column order
        let keys: Vec<&String> = table.rows[0].keys().collect();
        assert_eq!(keys, vec!["day", "n"]);
        assert_eq!(table.declared.get("day"), Some(&FieldType::Nominal));
        assert_eq!(table.declared.get("n"), Some(&FieldType::Quantitative));
    }

    #[test]
    fn duplicate_column_name_errors() {
        let doc = parse_data_doc(r#"{"columns":[{"name":"a"},{"name":"a"}],"rows":[[1,2]]}"#)
            .expect("parses");
        let err = resolve_stdin(doc).expect_err("duplicate must error");
        insta::assert_snapshot!(err.to_string(), @r###"duplicate column name "a""###);
    }

    #[test]
    fn row_length_mismatch_errors() {
        let doc =
            parse_data_doc(r#"{"columns":[{"name":"a"},{"name":"b"}],"rows":[[1,2],[1,2,3]]}"#)
                .expect("parses");
        let err = resolve_stdin(doc).expect_err("length mismatch must error");
        insta::assert_snapshot!(
            err.to_string(),
            @"row 1 has 3 values but 2 columns are declared"
        );
    }

    #[test]
    fn empty_rows_errors_columnar() {
        let doc = parse_data_doc(r#"{"columns":[{"name":"a"}],"rows":[]}"#).expect("parses");
        let err = resolve_stdin(doc).expect_err("empty rows must error");
        insta::assert_snapshot!(err.to_string(), @"data has no rows; provide at least one row");
    }

    #[test]
    fn empty_rows_errors_bare_array() {
        let doc = parse_data_doc(r#"[]"#).expect("parses");
        let err = resolve_stdin(doc).expect_err("empty rows must error");
        insta::assert_snapshot!(err.to_string(), @"data has no rows; provide at least one row");
    }

    #[test]
    fn empty_rows_errors_inline_values() {
        let err = resolve(&spec_with_empty_values(), None).expect_err("empty inline must error");
        insta::assert_snapshot!(err.to_string(), @"data has no rows; provide at least one row");
    }

    #[test]
    fn inline_values_resolve_to_inline_provenance() {
        let table = resolve(&spec_with_values(), None).expect("resolves");
        assert_eq!(table.provenance.source, DataSource::InlineValues);
        assert_eq!(table.provenance.truncated, None);
        assert_eq!(table.provenance.total_rows, None);
        assert!(table.declared.is_empty());
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn data_provided_twice_errors() {
        let doc = parse_data_doc(r#"[{"a":1}]"#).expect("parses");
        let err = resolve(&spec_with_values(), Some(doc)).expect_err("data twice must error");
        insta::assert_snapshot!(
            err.to_string(),
            @"data provided twice: the spec has inline `data` and a data document arrived on stdin; remove one"
        );
    }

    #[test]
    fn declared_field_type_mapping() {
        // Quantitative spellings.
        for t in [
            "INT64",
            "INTEGER",
            "INT",
            "SMALLINT",
            "BIGINT",
            "FLOAT64",
            "FLOAT",
            "DOUBLE",
            "NUMERIC",
            "BIGNUMERIC",
            "DECIMAL",
            "REAL",
        ] {
            assert_eq!(declared_field_type(t), FieldType::Quantitative, "{t}");
        }
        // Date/time spellings map to ordinal this cycle.
        for t in ["DATE", "DATETIME", "TIMESTAMP", "TIME"] {
            assert_eq!(declared_field_type(t), FieldType::Ordinal, "{t}");
        }
        // Strings and unknowns fall back to nominal.
        assert_eq!(declared_field_type("STRING"), FieldType::Nominal);
        assert_eq!(declared_field_type("BOOL"), FieldType::Nominal);
        assert_eq!(declared_field_type("whatever"), FieldType::Nominal);
        // Case-insensitive.
        assert_eq!(declared_field_type("int64"), FieldType::Quantitative);
        assert_eq!(declared_field_type("Date"), FieldType::Ordinal);
        assert_eq!(declared_field_type("String"), FieldType::Nominal);
    }
}
