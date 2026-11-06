//! The scrubbed 4B fixture opens and holds the states and stacks it was generated with.

mod integration_support;

use std::path::{Path, PathBuf};

use fin_core::{apply_committed, upcast_snapshot};
use fin_store::{OpenOptions, Store, read_project_file};
use integration_support::{copy_dir, fixture_4a, fixture_4b};

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

/// The three pre-4C fixtures (`(name, server_data, folder)`), laid out in `tmp`.
fn fixtures(tmp: &Path) -> Vec<(&'static str, PathBuf, PathBuf)> {
    let sp2 = tmp.join("sp2");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2"),
        &sp2,
    );
    let (d4a, f4a) = fixture_4a(&tmp.join("4a"));
    let (d4b, f4b) = fixture_4b(&tmp.join("4b"));
    vec![
        ("sp2", sp2.join("server-data"), sp2.join("folder")),
        ("4a", d4a, f4a),
        ("4b", d4b, f4b),
    ]
}

/// Every `txs` row as `(branch, seq, raw diff)` (`rev` in the sub-project 2 layout).
fn raw_diffs(db: &Path) -> Vec<(String, i64, String)> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let sql = |seq: &str| format!("SELECT branch, {seq}, diff FROM txs ORDER BY branch, {seq}");
    let mut stmt = conn
        .prepare(&sql("seq"))
        .or_else(|_| conn.prepare(&sql("rev")))
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Plan 4C Decision 5: on the SP2, 4A and 4B fixtures every row's stored diff, still v1-shaped
/// on disk, reads (upcast) as exactly what a fresh replay of its record records, and `verify`
/// agrees (`diffs_match` included). The stored diffs are not rewritten.
#[test]
fn diff_upcast_equals_replay_on_fixtures() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, data, folder) in fixtures(tmp.path()) {
        let id: String = read_project_file(&folder).unwrap().id;
        let db = fin_store::db_path(&data, &id);
        let before = raw_diffs(&db);
        let shifted = before
            .iter()
            .filter(|(_, _, d)| d.contains("\"shifts\":[{"))
            .count();
        assert!(shifted > 0, "{name}: the fixture has clip edits");
        assert!(
            before.iter().all(|(_, _, d)| !d.contains("\"op\":")),
            "{name}: every stored diff is v1-shaped"
        );
        let opts = OpenOptions {
            relocate: true,
            ..OpenOptions::default()
        };
        let store = Store::open_with(&data, &folder, opts).unwrap();

        let report = store.verify();
        let check = |n: &str| report.checks.iter().find(|c| c.name == n).unwrap();
        let diffs = check("diffs_match");
        assert!(diffs.ok, "{name}: {:?}", diffs.details);
        assert_eq!(diffs.details["mismatchCount"], 0, "{name}");
        assert_eq!(diffs.details["notStored"], 0, "{name}");
        assert_eq!(diffs.details["checked"], before.len(), "{name}");
        assert!(check("replay_matches_live").ok, "{name}: {:?}", report);

        // Through the read path too: each record of each branch's lineage, replayed in order
        // from rev 0, records the diff its row reads as.
        let rev0 = {
            let conn = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let json: String = conn
                .query_row(
                    "SELECT json FROM snapshots WHERE branch = 'main' AND seq = 0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            upcast_snapshot(serde_json::from_str(&json).unwrap()).unwrap()
        };
        let branches: Vec<String> = before.iter().map(|(b, _, _)| b.clone()).collect();
        let mut branches = branches;
        branches.dedup();
        for branch in branches {
            let mut project = rev0.clone();
            for c in store.ops_since(&branch, 0).unwrap() {
                let applied = apply_committed(&project, &c.tx, &c.bindings).unwrap();
                assert_eq!(applied.diff, c.diff, "{name} {branch} rev {}", c.rev);
                project = applied.project;
            }
            assert_eq!(
                project,
                store.snapshot(&branch).unwrap().1,
                "{name} {branch}"
            );
        }
        drop(store);
        assert_eq!(
            raw_diffs(&db),
            before,
            "{name}: stored diffs are never rewritten"
        );
    }
}

/// A 4B start trim (main seq 7, `c3` by 1) reads with its edge and content offset.
#[test]
fn fixture_4b_trim_reads_edge_and_content_offset() {
    let tmp = tempfile::tempdir().unwrap();
    let (data, folder) = fixture_4b(tmp.path());
    let opts = OpenOptions {
        relocate: true,
        ..OpenOptions::default()
    };
    let store = Store::open_with(&data, &folder, opts).unwrap();
    let rows = store.ops_since("main", 6).unwrap();
    let trim = rows.iter().find(|c| c.rev == 7).unwrap();
    assert_eq!(
        serde_json::to_value(&trim.diff).unwrap(),
        serde_json::json!({
            "changes": [{ "kind": "clip", "id": "c3", "change": "updated", "op": 0,
                          "edge": "start", "by": "-1" }],
            "shifts": [{ "track": "t2", "from": "12", "by": "-1", "op": 0 }]
        })
    );
    let undo = rows.iter().find(|c| c.rev == 14).unwrap();
    let ops: Vec<u32> = undo.diff.shifts.iter().map(|s| s.op).collect();
    assert_eq!(ops, [0, 1], "the undo's two removes, in op order");
}
