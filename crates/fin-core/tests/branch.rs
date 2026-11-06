//! The branch index and name rules (plan 4B Task 8, Decisions 11–13).

use std::collections::BTreeMap;

use fin_core::branch::{
    Absorbed, BranchIndex, BranchInfo, BranchState, id_number, validate_branch_name,
    validate_checkpoint_name,
};
use fin_core::{Link, Record, SCHEMA_VERSION, Tx};

fn record(branch: &str, seq: u64, link: Link) -> Record {
    let intent = Tx {
        tx_id: format!("{branch}-{seq}"),
        author: "ann".into(),
        branch: branch.into(),
        base_rev: seq - 1,
        schema_version: SCHEMA_VERSION,
        ops: vec![],
        args_hash: None,
    };
    let mut r = Record::new_edit(
        intent.tx_id.clone(),
        seq,
        SCHEMA_VERSION,
        None,
        "ann".into(),
        1_000 + seq as i64,
        intent,
        vec![],
        vec![],
        BTreeMap::new(),
    );
    r.set_link(link);
    r
}

fn branch(id: &str, name: &str, fork_seq: u64) -> Link {
    Link::Branch {
        branch_id: id.into(),
        name: name.into(),
        fork_seq,
        fork_hash: (fork_seq > 0).then(|| format!("h{fork_seq}")),
        epoch: 1,
        origin_hash: None,
    }
}

fn set(id: &str, name: Option<&str>, archived: Option<bool>) -> Link {
    Link::BranchSet {
        branch_id: id.into(),
        name: name.map(String::from),
        archived,
        origin_hash: None,
    }
}

fn info(id: &str, name: &str) -> BranchInfo {
    BranchInfo {
        id: id.into(),
        name: name.into(),
        parent: Some("main".into()),
        fork_seq: 0,
        fork_hash: None,
        created_seq: Some(1),
        created_by: "ann".into(),
        created_at: 0,
        record: None,
        epoch: 0,
        archived: false,
        merged: false,
    }
}

#[test]
fn branch_names_table() {
    for ok in [
        "feature",
        "Feature-2",
        "a",
        "v1.2_rc",
        "try-3",
        "recovered-0123abcd-20261105-101500",
        "brx",
        "br",
        "br1a",
        "main2",
        &"x".repeat(64),
    ] {
        assert_eq!(validate_branch_name(ok), Ok(()), "{ok:?}");
    }
    for bad in [
        "",
        &"x".repeat(65),
        "-x",
        ".x",
        "a b",
        "a/b",
        "a#b",
        "naïve",
        "main",
        "MAIN",
        "Main",
        "br1",
        "br10",
        "BR7",
        "br007",
    ] {
        let e = validate_branch_name(bad).unwrap_err();
        assert_eq!(e.code, "INVALID_NAME", "{bad:?}");
        assert_eq!(e.details["name"], bad);
        assert_eq!(e.details["kind"], "branch");
        assert!(e.fix.is_some());
    }
}

#[test]
fn checkpoint_names_table() {
    for ok in ["before-cut", "v1", "1a", "a.1"] {
        assert_eq!(validate_checkpoint_name(ok), Ok(()), "{ok:?}");
    }
    for bad in ["", "12", "0", "-a", ".a", "a b", "main", "br3"] {
        let e = validate_checkpoint_name(bad).unwrap_err();
        assert_eq!(e.code, "INVALID_NAME", "{bad:?}");
        assert_eq!(e.details["kind"], "checkpoint");
    }
}

#[test]
fn id_numbers_order_numerically() {
    assert_eq!(id_number("br10"), Some(10));
    assert_eq!(id_number("br9"), Some(9));
    assert_eq!(id_number("main"), None);
    assert_eq!(id_number("br"), None);
    assert_eq!(id_number("br1x"), None);
}

#[test]
fn index_folds_branch_and_branch_set() {
    let mut ix = BranchIndex::with_main();
    assert_eq!(ix.list().len(), 1);
    assert_eq!(
        ix.absorb(
            "main",
            3,
            &record("main", 3, branch("br1", "feature", 2)),
            "h"
        )
        .unwrap(),
        Absorbed::Made("br1".to_string())
    );
    let b = ix.get("br1").unwrap();
    assert_eq!(
        b,
        &BranchInfo {
            id: "br1".into(),
            name: "feature".into(),
            parent: Some("main".into()),
            fork_seq: 2,
            fork_hash: Some("h2".into()),
            created_seq: Some(3),
            created_by: "ann".into(),
            created_at: 1_003,
            record: Some("h".into()),
            epoch: 1,
            archived: false,
            merged: false,
        }
    );
    assert_eq!(b.state(), BranchState::Active);

    // A child of br1, then a rename and an archive of it, on br1's log.
    ix.absorb("br1", 4, &record("br1", 4, branch("br2", "deep", 3)), "h")
        .unwrap();
    ix.absorb(
        "br1",
        5,
        &record("br1", 5, set("br2", Some("deeper"), None)),
        "h",
    )
    .unwrap();
    assert_eq!(ix.get("br2").unwrap().name, "deeper");
    ix.absorb(
        "br1",
        6,
        &record("br1", 6, set("br2", None, Some(true))),
        "h",
    )
    .unwrap();
    assert_eq!(ix.get("br2").unwrap().state(), BranchState::Archived);
    assert!(ix.is_descendant("br2", "main"));
    assert!(ix.is_descendant("br2", "br1"));
    assert!(!ix.is_descendant("br1", "br2"));
    assert!(!ix.is_descendant("br1", "br1"));
    assert_eq!(ix.parents_first(), vec!["main", "br1", "br2"]);

    // Records of other kinds leave the index alone.
    let edit = record("br1", 7, Link::Edit);
    assert_eq!(ix.absorb("br1", 7, &edit, "h").unwrap(), Absorbed::Nothing);

    // Shapes the log never holds: a known id again, a branchSet off the parent's log or
    // of an unknown branch, a fork at or above the branch record.
    for (on, seq, link) in [
        ("main", 8, branch("br1", "again", 1)),
        ("main", 8, set("br2", Some("x"), None)),
        ("main", 8, set("br99", Some("x"), None)),
        ("main", 8, branch("br3", "high", 8)),
        ("main", 8, branch("main", "x", 1)),
        ("br77", 8, branch("br3", "x", 1)),
        // Names that would not resolve or list back, and fork hashes against Decision 3.
        ("main", 8, branch("br3", "br4", 1)),
        ("main", 8, branch("br3", "a#br1", 1)),
        ("main", 8, branch("br3", "MAIN", 1)),
        ("main", 8, set("br1", Some("br9"), None)),
        (
            "main",
            8,
            Link::Branch {
                branch_id: "br3".into(),
                name: "x".into(),
                fork_seq: 0,
                fork_hash: Some("h0".into()),
                epoch: 1,
                origin_hash: None,
            },
        ),
        (
            "main",
            8,
            Link::Branch {
                branch_id: "br3".into(),
                name: "x".into(),
                fork_seq: 2,
                fork_hash: None,
                epoch: 1,
                origin_hash: None,
            },
        ),
    ] {
        let e = ix
            .absorb(on, seq, &record(on, seq, link.clone()), "h")
            .unwrap_err();
        assert_eq!(e.code, "INVALID_FORMAT", "{link:?} on {on}");
    }
}

#[test]
fn parents_first_is_depth_then_numeric_id() {
    let mut ix = BranchIndex::with_main();
    ix.absorb("main", 1, &record("main", 1, branch("br10", "a", 0)), "h")
        .unwrap();
    ix.absorb("main", 2, &record("main", 2, branch("br9", "b", 0)), "h")
        .unwrap();
    ix.absorb("br9", 3, &record("br9", 3, branch("br11", "c", 2)), "h")
        .unwrap();
    ix.absorb("br10", 4, &record("br10", 4, branch("br2", "d", 1)), "h")
        .unwrap();
    assert_eq!(
        ix.parents_first(),
        vec!["main", "br9", "br10", "br2", "br11"]
    );
    let ids: Vec<&str> = ix.list().iter().map(|(_, i)| i.id.as_str()).collect();
    assert_eq!(ids, vec!["main", "br2", "br9", "br10", "br11"]);
}

#[test]
fn duplicate_names_lower_numeric_id_keeps_name() {
    // Arriving through import: both are active under one name (case-insensitively).
    let mut ix = BranchIndex::with_main();
    ix.absorb("main", 1, &record("main", 1, branch("br10", "Cut", 0)), "h")
        .unwrap();
    ix.absorb("main", 2, &record("main", 2, branch("br9", "cut", 0)), "h")
        .unwrap();
    assert_eq!(ix.resolve("cut").unwrap().id, "br9");
    assert_eq!(ix.resolve("CUT").unwrap().id, "br9");
    assert_eq!(ix.resolve("Cut#br10").unwrap().id, "br10");
    assert_eq!(ix.resolve("cut#br10").unwrap().id, "br10");
    let display: Vec<(String, String)> = ix
        .list()
        .into_iter()
        .map(|(d, i)| (d, i.id.clone()))
        .collect();
    assert_eq!(
        display,
        vec![
            ("main".to_string(), "main".to_string()),
            ("cut".to_string(), "br9".to_string()),
            ("Cut#br10".to_string(), "br10".to_string()),
        ]
    );

    // Archiving the holder hands the name to the other one.
    ix.absorb(
        "main",
        3,
        &record("main", 3, set("br9", None, Some(true))),
        "h",
    )
    .unwrap();
    assert_eq!(ix.resolve("cut").unwrap().id, "br10");
    let display: Vec<String> = ix.list().into_iter().map(|(d, _)| d).collect();
    assert_eq!(display, vec!["main", "cut#br9", "Cut"]);
}

#[test]
fn resolve_forms() {
    let mut ix = BranchIndex::with_main();
    ix.absorb(
        "main",
        1,
        &record("main", 1, branch("br1", "feature", 0)),
        "h",
    )
    .unwrap();
    ix.absorb("main", 2, &record("main", 2, branch("br2", "old", 0)), "h")
        .unwrap();
    ix.absorb(
        "main",
        3,
        &record("main", 3, set("br2", None, Some(true))),
        "h",
    )
    .unwrap();
    assert_eq!(ix.resolve("main").unwrap().id, "main");
    assert_eq!(ix.resolve("br1").unwrap().id, "br1");
    assert_eq!(ix.resolve("feature").unwrap().id, "br1");
    assert_eq!(ix.resolve("Feature").unwrap().id, "br1");
    assert_eq!(ix.resolve("feature#br1").unwrap().id, "br1");
    // Archived: by id or name#id only.
    assert!(ix.resolve("old").is_none());
    assert_eq!(ix.resolve("br2").unwrap().id, "br2");
    assert_eq!(ix.resolve("old#br2").unwrap().id, "br2");
    // A name#id whose parts disagree, an unknown id, an unknown name.
    assert!(ix.resolve("feature#br2").is_none());
    assert!(ix.resolve("feature#br3").is_none());
    assert!(ix.resolve("br3").is_none());
    assert!(ix.resolve("nope").is_none());
    assert!(!ix.name_taken("old"));
    assert!(ix.name_taken("FEATURE"));
}

#[test]
fn merged_counts_as_holding_its_name() {
    let mut merged = info("br1", "done");
    merged.merged = true;
    let mut archived = info("br2", "gone");
    archived.archived = true;
    archived.merged = true;
    let ix = BranchIndex::from_infos([merged, archived]);
    assert!(ix.get("main").is_some(), "main is always in the index");
    assert_eq!(ix.get("br1").unwrap().state(), BranchState::Merged);
    assert_eq!(ix.get("br2").unwrap().state(), BranchState::Archived);
    assert!(ix.name_taken("done"));
    assert_eq!(ix.resolve("done").unwrap().id, "br1");
    assert!(!ix.name_taken("gone"));
}

#[test]
fn branch_state_serialises_camel_case() {
    assert_eq!(
        serde_json::to_value([
            BranchState::Active,
            BranchState::Archived,
            BranchState::Merged
        ])
        .unwrap(),
        serde_json::json!(["active", "archived", "merged"])
    );
}

fn branch_at(id: &str, name: &str, fork_seq: u64, epoch: u64) -> Link {
    Link::Branch {
        branch_id: id.into(),
        name: name.into(),
        fork_seq,
        fork_hash: (fork_seq > 0).then(|| format!("h{fork_seq}")),
        epoch,
        origin_hash: None,
    }
}

// Decision 33: the same id made by two `branch` records (two servers across a takeover) is
// held by the higher epoch, then the lower txHash, whatever order they are folded in; the
// loser's descendants go with it.
#[test]
fn id_collision_is_held_by_higher_epoch_then_lower_hash() {
    let x = ("main", 2, branch_at("br1", "x-side", 1, 1), "hx");
    let y = ("main", 3, branch_at("br1", "y-side", 1, 2), "hy");
    let fold = |order: &[&(&str, u64, Link, &str)]| {
        let mut ix = BranchIndex::with_main();
        let mut out = vec![];
        for (on, seq, link, hash) in order {
            out.push(
                ix.absorb(on, *seq, &record(on, *seq, link.clone()), hash)
                    .unwrap(),
            );
        }
        (ix, out)
    };
    let (a, out) = fold(&[&x, &y]);
    assert!(
        matches!(&out[1], Absorbed::Replaced { id, old } if id == "br1" && old.name == "x-side")
    );
    let (b, out) = fold(&[&y, &x]);
    assert_eq!(
        out[1],
        Absorbed::Lost {
            id: "br1".into(),
            winner: "hy".into()
        }
    );
    assert_eq!(a, b);
    assert_eq!(a.get("br1").unwrap().name, "y-side");
    assert_eq!(a.get("br1").unwrap().record.as_deref(), Some("hy"));

    // Equal epochs: the lower txHash holds it.
    let p = ("main", 2, branch_at("br1", "p", 1, 2), "hb");
    let q = ("main", 3, branch_at("br1", "q", 1, 2), "ha");
    assert_eq!(fold(&[&p, &q]).0, fold(&[&q, &p]).0);
    assert_eq!(fold(&[&p, &q]).0.get("br1").unwrap().name, "q");

    // A replaced branch's children are gone from the index with it.
    let child = ("br1", 3, branch_at("br2", "kid", 2, 1), "hk");
    let x2 = ("main", 2, branch_at("br1", "x-side", 1, 1), "hx");
    let (ix, _) = fold(&[&x2, &child, &y]);
    assert!(ix.get("br2").is_none());
}
