//! The scrubbed 4A fixture opens and holds the state it was generated with.

mod integration_support;

use fin_store::{MAIN, OpenOptions, Store};
use integration_support::fixture_4a;

#[test]
fn fixture_4a_opens_with_relocate() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, folder) = fixture_4a(tmp.path());
    let opts = OpenOptions {
        relocate: true,
        ..OpenOptions::default()
    };
    let store = Store::open_with(&data, &folder, opts).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/4a/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let (rev, project) = store.snapshot(MAIN).unwrap();
    assert_eq!(rev, expected["rev"].as_u64().unwrap());
    assert_eq!(serde_json::to_value(&project).unwrap(), expected["project"]);
}
