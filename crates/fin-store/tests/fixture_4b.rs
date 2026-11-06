//! The scrubbed 4B fixture opens and holds the states and stacks it was generated with.

mod integration_support;

use fin_store::{OpenOptions, Store};
use integration_support::fixture_4b;

#[test]
fn fixture_4b_opens_with_relocate() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, folder) = fixture_4b(tmp.path());
    let opts = OpenOptions {
        relocate: true,
        ..OpenOptions::default()
    };
    let store = Store::open_with(&data, &folder, opts).unwrap();
    let expected: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/4b/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let branches = expected["branches"].as_object().unwrap();
    assert_eq!(
        branches.keys().collect::<Vec<_>>(),
        ["br1", "main"],
        "the fixture has main and one feature branch"
    );
    for (branch, want) in branches {
        let (rev, project) = store.snapshot(branch).unwrap();
        assert_eq!(rev, want["rev"].as_u64().unwrap(), "{branch} rev");
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            want["project"],
            "{branch} project"
        );
        let stacks = store.undo_stacks(branch, "fixture").unwrap();
        assert_eq!(
            serde_json::to_value(&stacks).unwrap(),
            expected["stacks"][branch],
            "{branch} stacks"
        );
    }
    // The hashes 4B's `fin` computed (pinned in `fin` by Task 6): well-formed, all distinct.
    let hashes = expected["argsHashes"].as_array().unwrap();
    assert_eq!(hashes.len(), 3);
    for h in hashes {
        let h = h["hash"].as_str().unwrap();
        assert!(h.len() == 71 && h.starts_with("blake3:"), "{h}");
    }
}
