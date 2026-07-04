//! CLI stdin-routing integration tests. Exercises the whole binary: where the
//! spec comes from decides stdin's role (spec vs data document), plus the
//! precedence and redirect errors that live one layer down in core.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// A spec with NO inline data — expects its rows to arrive on stdin.
const SPEC_NO_DATA: &str = r#"{"mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#;

/// A columnar envelope as an MCP producer would emit it: known fields plus an
/// unknown `query` provenance block that benday must ignore, and `truncated`.
const ENVELOPE: &str = r#"{
    "columns": [{"name":"m","type":"STRING"},{"name":"v","type":"INT64"}],
    "rows": [["a",3],["b",7]],
    "total_rows": 123,
    "truncated": true,
    "query": {"job_id":"abc","note":"ignored"}
}"#;

fn benday() -> Command {
    Command::cargo_bin("benday").expect("binary builds")
}

#[test]
fn pipe_envelope_renders() {
    benday()
        .args(["--spec", SPEC_NO_DATA, "--meta", "--no-color"])
        .write_stdin(ENVELOPE)
        .assert()
        .success()
        // meta goes to stderr; serde_json Value Display is compact, no spaces.
        .stdout(predicates::str::is_empty().not())
        .stderr(contains(r#""truncated":true"#));
}

#[test]
fn pipe_bare_array_renders() {
    benday()
        .args(["--spec", SPEC_NO_DATA])
        .write_stdin(r#"[{"m":"a","v":3},{"m":"b","v":7}]"#)
        .assert()
        .success();
}

#[test]
fn spec_on_stdin_still_works() {
    // No flags: stdin IS the spec, with inline values — today's behavior.
    let spec = r#"{"data":{"values":[{"m":"a","v":3},{"m":"b","v":7}]},
                   "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#;
    benday().write_stdin(spec).assert().success();
}

#[test]
fn data_twice_fails() {
    let spec_with_data = r#"{"data":{"values":[{"m":"a","v":3}]},
                            "mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}"#;
    benday()
        .args(["--spec", spec_with_data])
        .write_stdin(ENVELOPE)
        .assert()
        .code(2)
        .stderr(contains("data provided twice"));
}

#[test]
fn forgotten_spec_flag_redirects() {
    // Envelope piped with no --spec: stdin parses as a spec but smells like data.
    benday()
        .write_stdin(ENVELOPE)
        .assert()
        .code(2)
        .stderr(contains("looks like a data document"));
}

#[test]
fn no_data_anywhere_fails() {
    // --spec without data and empty stdin: nothing to chart.
    benday()
        .args(["--spec", SPEC_NO_DATA])
        .write_stdin("")
        .assert()
        .code(2)
        .stderr(contains("no data"));
}

#[test]
fn length_mismatch_exit_3() {
    // Three declared columns, a row with only two values → transposition guard.
    let short_row = r#"{"columns":[{"name":"m"},{"name":"v"},{"name":"w"}],
                        "rows":[["a",3]]}"#;
    benday()
        .args(["--spec", SPEC_NO_DATA])
        .write_stdin(short_row)
        .assert()
        .code(3)
        .stderr(contains("2 values but 3 columns"));
}

#[test]
fn dump_scene_shows_stdin_source() {
    benday()
        .args(["--spec", SPEC_NO_DATA, "--dump-scene"])
        .write_stdin(ENVELOPE)
        .assert()
        .success()
        .stdout(contains(r#""stdin_columns""#));
}
