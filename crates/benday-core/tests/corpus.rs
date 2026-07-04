//! Spec→Scene golden corpus. Each tests/cases/*.json is compiled at the
//! default size; the Scene JSON (or the error) is the snapshot. This is the
//! primary regression contract — semantic, layout-aware, glyph-free.

use std::fs;
use std::path::Path;

use benday_core::compile::{compile, CompileOptions};
use benday_core::{spec::Spec, theme};

#[test]
fn corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 15, "corpus shrank: {} cases", paths.len());
    let opts = CompileOptions {
        width: None,
        height: None,
        theme: theme::by_name("benday").unwrap(),
    };
    for path in &paths {
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let spec: Spec = serde_json::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|e| panic!("{stem}: case must be a parseable spec: {e}"));
        let snapshot = match compile(&spec, &opts) {
            Ok(scene) => scene.to_json(),
            Err(e) => format!("ERROR ({}): {}", e.kind(), e),
        };
        insta::assert_snapshot!(format!("case__{stem}"), snapshot);
    }
}
