//! The scrubbed 4B fixture opens and holds the states and stacks it was generated with (the
//! states in model v2: `expected_v2.json`).

mod integration_support;

use std::path::{Path, PathBuf};

use fin_core::{
    InsertMode, ItemBody, ItemInsert, Op, Project, Rational, SCHEMA_VERSION, Tx, apply_committed,
    upcast_snapshot,
};
use fin_store::{CherryPickReq, HistoryReq, MAIN, MergeReq, OpenOptions, Store, read_project_file};
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
    let states = expected_v2("4b");
    let branches = states["branches"].as_object().unwrap();
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

/// A fixture's `expected_v2.json`: its states (`{rev, project}`, per branch for 4B) in model
/// v2, generated once with `upcast_snapshot` from `expected.json`, reviewed by hand and frozen.
fn expected_v2(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .join("expected_v2.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// `(branch, {rev, project})` of a fixture's expected file, `main` alone for SP2 and 4A.
fn states(name: &str, v: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    match v.get("branches") {
        Some(b) => b
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect(),
        None => {
            assert!(name != "4b");
            vec![("main".to_string(), v.clone())]
        }
    }
}

/// Plan 4C Task 6: every pre-4C fixture opens in model v2 with the states of its frozen
/// `expected_v2.json`, and those are the upcasts of the v1 states 4B (or earlier) recorded in
/// `expected.json`: same ids, times and assets, only shape and defaults differ.
#[test]
fn sp2_4a_4b_fixtures_open_equal_v2_goldens() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, data, folder) in fixtures(tmp.path()) {
        let v1: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(name)
                    .join("expected.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let v2 = expected_v2(name);
        let (v1, v2) = (states(name, &v1), states(name, &v2));
        assert_eq!(v1.len(), v2.len(), "{name}");
        let opts = OpenOptions {
            relocate: true,
            ..OpenOptions::default()
        };
        let store = Store::open_with(&data, &folder, opts).unwrap();
        for ((b1, old), (b2, new)) in v1.iter().zip(&v2) {
            assert_eq!(b1, b2, "{name}");
            assert_eq!(old["project"]["schemaVersion"], 1, "{name} {b1}");
            assert_eq!(new["project"]["schemaVersion"], 2, "{name} {b1}");
            assert_eq!(old["rev"], new["rev"], "{name} {b1}");
            let up = upcast_snapshot(old["project"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(&up).unwrap(),
                new["project"],
                "{name} {b1}"
            );
            let (rev, project) = store.snapshot(b1).unwrap();
            assert_eq!(rev, new["rev"].as_u64().unwrap(), "{name} {b1}");
            assert_eq!(project, up, "{name} {b1}");
            assert_eq!(serde_json::to_value(&project).unwrap(), new["project"]);
        }
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
        // The genesis check and every other check pass too (review of Task 6).
        assert!(report.ok, "{name}: {report:?}");

        // Through the read path too: each record of each branch's lineage, replayed in order
        // from rev 0, records the diff its row reads as.
        let rev0 = rev0(&db);
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
            "changes": [{ "kind": "item", "id": "c3", "change": "updated", "op": 0,
                          "edge": "start", "by": "-1" }],
            "shifts": [{ "track": "t2", "from": "12", "by": "-1", "op": 0 }]
        })
    );
    let undo = rows.iter().find(|c| c.rev == 14).unwrap();
    let ops: Vec<u32> = undo.diff.shifts.iter().map(|s| s.op).collect();
    assert_eq!(ops, [0, 1], "the undo's two removes, in op order");
}

/// The rev-0 project a database holds (upcast).
fn rev0(db: &Path) -> Project {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let json: String = conn
        .query_row(
            "SELECT json FROM snapshots WHERE branch = 'main' AND seq = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    upcast_snapshot(serde_json::from_str(&json).unwrap()).unwrap()
}

/// `branch` replayed from rev 0 through its records up to `rev`, each upcast as read.
fn replay_to(store: &Store, db: &Path, branch: &str, rev: u64) -> Project {
    let mut project = rev0(db);
    for c in store.ops_since(branch, 0).unwrap() {
        if c.rev > rev {
            break;
        }
        project = apply_committed(&project, &c.tx, &c.bindings)
            .unwrap()
            .project;
    }
    project
}

/// The 4B fixture, open for writing (a fixed lease past its records' epoch 1), and its DB.
fn writable_4b(tmp: &Path) -> (Store, PathBuf) {
    let (data, folder) = fixture_4b(tmp);
    let opts = OpenOptions {
        relocate: true,
        ..OpenOptions::default()
    };
    let mut store = Store::open_with(&data, &folder, opts).unwrap();
    store.set_test_lease(2, true);
    let db = fin_store::db_path(&data, &read_project_file(&folder).unwrap().id);
    (store, db)
}

fn assert_verifies(store: &Store) {
    let report = store.verify();
    assert!(report.ok, "{report:?}");
}

fn no_alias(ops: &[Op]) {
    assert!(ops.iter().all(|o| !o.is_alias()), "{ops:?}");
}

/// Plan 4C Task 7: 4B's records undo after the upgrade. The fixture's last two `main` units
/// (its cherry-pick, then the redo of its undone merge) undo as `item.*` inverses, to the
/// state of `main` before them; a redo brings the pick back.
#[test]
fn undo_of_4b_record_after_upgrade() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, db) = writable_4b(tmp.path());
    let head = s.snapshot(MAIN).unwrap().1;
    let req = |id: &str| HistoryReq {
        tx_id: id.into(),
        author: "fixture".into(),
        branch: MAIN.into(),
        dry_run: false,
        args_hash: None,
        to: None,
    };
    let u1 = s.undo(req("u-4c-1")).unwrap();
    assert_eq!(u1.undone, ["01JC0000000000000000000018"], "the cherry-pick");
    no_alias(&u1.committed[0].tx.ops);
    assert_eq!(s.snapshot(MAIN).unwrap().1, replay_to(&s, &db, MAIN, 15));
    let u2 = s.undo(req("u-4c-2")).unwrap();
    assert_eq!(u2.undone, ["01JC0000000000000000000016"], "4B's redo");
    let names: Vec<&str> = u2.committed[0].tx.ops.iter().map(Op::name).collect();
    assert_eq!(names, ["item.remove", "item.remove"]);
    assert_eq!(s.snapshot(MAIN).unwrap().1, replay_to(&s, &db, MAIN, 11));
    s.redo(req("r-4c-1")).unwrap();
    s.redo(req("r-4c-2")).unwrap();
    assert_eq!(s.snapshot(MAIN).unwrap().1, head);
    assert_verifies(&s);
}

/// Plan 4C Task 7: a 4B branch merges after the upgrade, with a 4C edit on top of its 4B
/// records.
#[test]
fn merge_of_4b_branch_after_upgrade() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _) = writable_4b(tmp.path());
    let br1 = s.snapshot("br1").unwrap().1;
    let end = br1.timelines[0].tracks[0]
        .items
        .iter()
        .map(|i| i.end().unwrap())
        .max()
        .unwrap();
    let insert = Op::from(ItemInsert {
        id: "$c".into(),
        track: "t1".into(),
        at: end,
        duration: Rational::from_int(1),
        mode: InsertMode::Ripple,
        body: ItemBody::Clip {
            asset: "a1".into(),
            source_in: Rational::zero(),
            speed: Rational::from_int(1),
        },
    });
    let c = s
        .commit(Tx {
            tx_id: "e-4c".into(),
            author: "fixture".into(),
            branch: "br1".into(),
            base_rev: s.head("br1"),
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![insert],
        })
        .unwrap();
    let new_id = c.bindings["$c"].clone();
    let m = s
        .merge(MergeReq {
            tx_id: "m-4c".into(),
            author: "fixture".into(),
            from: "br1".into(),
            into: MAIN.into(),
            skip_conflicting: false,
            dry_run: false,
            args_hash: None,
        })
        .unwrap();
    assert!(m.skipped.is_empty(), "{m:?}");
    assert!(m.merged.contains(&"e-4c".to_string()), "{m:?}");
    for c in &m.committed {
        no_alias(&c.tx.ops);
    }
    let main = s.snapshot(MAIN).unwrap().1;
    assert!(
        main.timelines[0].tracks[0]
            .items
            .iter()
            .any(|i| i.id == new_id)
    );
    assert_verifies(&s);
}

/// Plan 4C Task 7: a 4B record cherry-picks after the upgrade: `main`'s insert after the
/// fork, onto `br1`.
#[test]
fn cherry_pick_of_4b_record_after_upgrade() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, _) = writable_4b(tmp.path());
    let count = |s: &Store| -> usize {
        let p = s.snapshot("br1").unwrap().1;
        p.timelines[0].tracks.iter().map(|t| t.items.len()).sum()
    };
    let before = count(&s);
    let r = s
        .cherry_pick(CherryPickReq {
            tx_id: "p-4c".into(),
            author: "fixture".into(),
            origin: "01JC0000000000000000000013".into(),
            into: "br1".into(),
            dry_run: false,
            args_hash: None,
        })
        .unwrap();
    assert_eq!(r.committed.len(), 1);
    let names: Vec<&str> = r.committed[0].tx.ops.iter().map(Op::name).collect();
    assert_eq!(names, ["item.insert"]);
    assert_eq!(count(&s), before + 1);
    assert_verifies(&s);
}

/// Plan 4C Task 7: every pre-4C fixture's records, upcast and replayed from rev 0, give the
/// frozen v2 goldens; the 4A and 4B folders (which hold segments) rebuild from the folder
/// alone to the same states. (The SP2 fixture has no folder segments: its database is the
/// only copy of its history.)
#[test]
fn replay_of_upcast_records_equals_v2_golden() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, data, folder) in fixtures(tmp.path()) {
        let golden = expected_v2(name);
        let opts = || OpenOptions {
            relocate: true,
            ..OpenOptions::default()
        };
        let store = Store::open_with(&data, &folder, opts()).unwrap();
        let db = fin_store::db_path(&data, &read_project_file(&folder).unwrap().id);
        for (branch, want) in states(name, &golden) {
            let rev = want["rev"].as_u64().unwrap();
            let replayed = replay_to(&store, &db, &branch, rev);
            assert_eq!(
                serde_json::to_value(&replayed).unwrap(),
                want["project"],
                "{name} {branch}"
            );
        }
        drop(store);
        if name == "sp2" {
            continue;
        }
        std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
        let store = Store::open_with(&data, &folder, opts()).unwrap();
        let report = store.last_rebuild().expect("rebuilt at open").clone();
        assert!(report.verify.ok, "{name}: {:?}", report.verify);
        for (branch, want) in states(name, &golden) {
            let (rev, project) = store.snapshot(&branch).unwrap();
            assert_eq!(rev, want["rev"].as_u64().unwrap(), "{name} {branch}");
            assert_eq!(
                serde_json::to_value(&project).unwrap(),
                want["project"],
                "{name} {branch}"
            );
        }
    }
}
