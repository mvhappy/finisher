//! Recovered branches (plan 4B Task 16, Decisions 5, 8, 27–29): the records set aside in
//! quarantine come back as `recovered-*` branches, under fresh ids; the recovered marks are
//! derived from the log, so followers and a rebuild agree.
//!
//! Setup as `tests/cascade.rs`: A (epoch 1) writes, pauses, B takes over (epoch 2); A's late
//! files reach the follower F before B's.

mod integration_support;

use std::fs;
use std::path::Path;

use fin_core::{
    CommittedTx, Edge, Link, Op, Project, Rational, Record, SCHEMA_VERSION, Tx, encode_record,
};
use fin_store::{
    BranchCreate, CheckpointReq, HistoryReq, MAIN, ReadOnlyReason, RecoverReport, RecoverReq,
    RecoverSel, Store,
};
use integration_support::diverge::*;
use integration_support::*;
use rusqlite::{Connection, params};

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
}

fn trim(clip: &str) -> Op {
    trim_item(clip, Edge::End, Rational::from_int(1))
}

fn branch(s: &mut Store, tx_id: &str, name: &str, from: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn req(tx_id: &str, create: Option<RecoverSel>) -> RecoverReq {
    RecoverReq {
        tx_id: tx_id.into(),
        author: "rec".into(),
        create,
        import_dir: None,
        dismiss: None,
        dry_run: false,
    }
}

fn create_all(s: &mut Store, tx_id: &str) -> RecoverReport {
    s.recover(req(tx_id, Some(RecoverSel::All))).unwrap()
}

fn db(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

fn tx_hash(s: &Store, tx_id: &str) -> String {
    db(s)
        .query_row("SELECT tx_hash FROM txs WHERE tx_id = ?1", [tx_id], |r| {
            r.get(0)
        })
        .unwrap()
}

/// `(txId, reason)` of the current quarantine rows, sorted.
fn quarantine(s: &Store) -> Vec<(String, String)> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT record_bytes, reason FROM quarantine WHERE current = 1")
        .unwrap();
    let mut out: Vec<(String, String)> = stmt
        .query_map([], |r| {
            let bytes: Vec<u8> = r.get(0)?;
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            Ok((v["txId"].as_str().unwrap().to_string(), r.get(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    out.sort();
    out
}

/// The rows status counts; checks that they are the rows the listing groups (status reads
/// the marks from `quarantine_marks`, the listing from the rows' bytes).
fn counted(s: &Store) -> u32 {
    let n = s.status().quarantine.count;
    let listed: usize = s.recover_groups().unwrap().iter().map(|g| g.records).sum();
    assert_eq!(
        n as usize, listed,
        "status and the listing agree on the marks"
    );
    n
}

fn assert_verifies(s: &Store) {
    let v = s.verify();
    assert!(v.ok, "{v:#?}");
}

/// Every clip id on `t1` of `p`, in order.
fn clips(p: &Project) -> Vec<String> {
    let t = p.timelines[0].tracks.iter().find(|t| t.id == "t1").unwrap();
    t.items.iter().map(|c| c.id.clone()).collect()
}

fn kinds(c: &[CommittedTx]) -> Vec<&'static str> {
    c.iter()
        .map(|c| match c.link.as_deref() {
            None | Some(Link::Edit) => "edit",
            Some(Link::Branch { .. }) => "branch",
            Some(Link::CherryPick { .. }) => "cherryPick",
            Some(Link::Checkpoint { .. }) => "checkpoint",
            Some(_) => "other",
        })
        .collect()
}

fn origins(c: &[CommittedTx]) -> Vec<(String, Option<String>)> {
    c.iter()
        .filter_map(|c| match c.link.as_deref() {
            Some(Link::CherryPick {
                origin, recovers, ..
            }) => Some((origin.clone(), recovers.clone())),
            _ => None,
        })
        .collect()
}

/// Checks the `recovered-<server8>-<YYYYMMDD-HHMMSS>` shape and the server part.
fn assert_name(name: &str, server: &str) {
    let rest = name
        .strip_prefix(&format!("recovered-{}-", &server[..8]))
        .unwrap_or_else(|| panic!("{name} for {server}"));
    let b = rest.as_bytes();
    assert!(b.len() >= 15, "{name}");
    assert!(
        b[..8].iter().all(u8::is_ascii_digit) && b[8] == b'-',
        "{name}"
    );
    assert!(b[9..15].iter().all(u8::is_ascii_digit), "{name}");
}

#[test]
fn diverged_tail_becomes_recovered_branch() {
    let e = env();
    let Split { mut a, mut f, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
        a.checkpoint(CheckpointReq {
            tx_id: "xc".into(),
            author: "test".into(),
            branch: MAIN.into(),
            name: "v1".into(),
            dry_run: false,
            args_hash: None,
        })
        .unwrap();
        on(a, "x2", MAIN, vec![insert(1, "a1")]);
    });
    let q = quarantine(&a);
    assert_eq!(q.len(), 3, "{q:?}");
    assert!(q.iter().all(|(_, r)| r == "DIVERGED_TAIL"));
    assert_eq!(counted(&a), 3);
    let w = a.status().warnings;
    let w = w.iter().find(|w| w.code == "QUARANTINE").unwrap();
    assert!(w.message.contains("fin project recover"), "{}", w.message);

    let groups = a.recover_groups().unwrap();
    assert_eq!(groups.len(), 1, "{groups:#?}");
    let g = &groups[0];
    assert_eq!(
        (g.branch.as_str(), g.fork_seq, g.records, g.origin.as_str()),
        (MAIN, 1, 3, "local")
    );
    assert_eq!(g.reasons.get("DIVERGED_TAIL"), Some(&3));
    assert_eq!(g.server, a.server_id());
    assert_eq!(g.fork_hash.as_deref(), Some(tx_hash(&a, "t0").as_str()));
    assert!(g.recoverable && g.problems.is_empty(), "{g:#?}");
    assert_name(&g.name, a.server_id());
    assert_eq!(g.id.len(), 12);

    let r = create_all(&mut a, "rec1");
    assert!(!r.dry_run);
    assert_eq!(r.groups, groups);
    assert_eq!(r.created.len(), 1);
    let made = &r.created[0];
    assert_eq!(
        (
            made.name.as_str(),
            made.parent.as_deref(),
            made.fork_seq,
            made.head
        ),
        (g.name.as_str(), Some(MAIN), 1, 4)
    );
    assert_eq!(
        kinds(&r.committed),
        ["branch", "cherryPick", "checkpoint", "cherryPick"]
    );
    let ids: Vec<&str> = r.committed.iter().map(|c| c.tx.tx_id.as_str()).collect();
    assert_eq!(ids, ["rec1-1", "rec1-2", "rec1-3", "rec1-4"]);
    assert!(r.committed[1..].iter().all(|c| c.tx.author == "rec"));
    let quarantined: Vec<(String, Option<String>)> = vec![
        ("x1".into(), Some(hash_of_quarantined(&a, "x1"))),
        ("x2".into(), Some(hash_of_quarantined(&a, "x2"))),
    ];
    assert_eq!(origins(&r.committed), quarantined);
    assert_eq!(a.find_checkpoint(&made.id, "v1").unwrap().1, 3);

    // Recovered: kept in quarantine, no longer counted or listed.
    assert_eq!(quarantine(&a).len(), 3);
    assert_eq!(counted(&a), 0);
    assert!(a.recover_groups().unwrap().is_empty());
    assert!(!a.status().warnings.iter().any(|w| w.code == "QUARANTINE"));
    assert_verifies(&a);

    // Followers see the branch.
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(f.snapshot(&made.id).unwrap(), a.snapshot(&made.id).unwrap());
    assert_eq!(f.branches(true).unwrap(), a.branches(true).unwrap());
    assert_verifies(&f);
}

/// The `txHash` of the quarantined record with `tx_id`.
fn hash_of_quarantined(s: &Store, tx_id: &str) -> String {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT tx_hash, record_bytes FROM quarantine")
        .unwrap();
    let rows: Vec<(String, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows.into_iter()
        .find(|(_, b)| {
            let v: serde_json::Value = serde_json::from_slice(b).unwrap();
            v["txId"] == tx_id
        })
        .map(|(h, _)| h)
        .unwrap_or_else(|| panic!("{tx_id} not in quarantine"))
}

/// Review focus 4: A's tail used `c1`; B, after the takeover, allocated `c1` too. The
/// recovered copy gets a new id, and the later record that named `c1` names the new one.
#[test]
fn recovered_tail_gets_fresh_ids_after_takeover() {
    let e = env();
    let Split { mut a, b, .. } = diverged(&e, |a| {
        let x1 = on(a, "x1", MAIN, vec![insert(0, "a1")]);
        assert_eq!(x1.bindings["$c"], "c1");
        on(a, "x2", MAIN, vec![trim("c1")]);
    });
    assert_eq!(clips(&b.snapshot(MAIN).unwrap().1), ["c1"], "B's c1");
    let r = create_all(&mut a, "rec");
    let made = &r.created[0];
    let copy = &r.committed[1];
    assert_ne!(copy.bindings["$c"], "c1");
    let fresh = copy.bindings["$c"].clone();
    assert_eq!(
        copy.tx.ops,
        [insert(0, "a1")],
        "re-run from its placeholder"
    );
    assert_eq!(r.committed[2].tx.ops, [trim(&fresh)]);
    let (_, p) = a.snapshot(&made.id).unwrap();
    assert_eq!(clips(&p), std::slice::from_ref(&fresh));
    let clip = &p.timelines[0].tracks[0].items[0];
    assert_eq!(
        clip.duration,
        Rational::from_int(2),
        "trimmed under its new id"
    );
    // `main` keeps B's c1; the next id this server hands out is past both.
    // (`main` is one record ahead: the recovered branch's `branch` record.)
    assert_eq!(a.snapshot(MAIN).unwrap().1, b.snapshot(MAIN).unwrap().1);
    let next = on(&mut a, "y1", MAIN, vec![insert(0, "a1")]);
    assert_ne!(next.bindings["$c"], fresh);
    assert_ne!(next.bindings["$c"], "c1");
    assert_verifies(&a);
}

/// Decision 27 as built: a literal id that names an entity deleted before the fork (an undo
/// brings it back) is the same entity, not a collision.
#[test]
fn restore_of_a_clip_deleted_before_the_fork_is_no_collision() {
    let e = env();
    let Split { mut a, .. } = split(
        &e,
        |a| {
            commit(a, "e1", vec![insert(0, "a1")]);
            commit(a, "e2", vec![delete_item("c1")]);
        },
        |_| {},
        winner,
    );
    a.undo(HistoryReq {
        tx_id: "x1".into(),
        author: "test".into(),
        branch: MAIN.into(),
        dry_run: false,
        args_hash: None,
        to: None,
        after_seq: None,
    })
    .unwrap();
    a.set_test_lease(1, false);
    e.poll(&mut a);
    assert_eq!(quarantine(&a), [("x1".into(), "DIVERGED_TAIL".into())]);
    a.set_test_lease(3, true);
    let g = a.recover_groups().unwrap();
    assert!(g[0].recoverable, "{g:#?}");
    let r = create_all(&mut a, "rec");
    let (_, p) = a.snapshot(&r.created[0].id).unwrap();
    assert_eq!(clips(&p), ["c1"], "the deleted clip is back under its id");
}

/// Inserts a hand-made record into `s`'s quarantine as a diverged tail.
fn craft(
    s: &Store,
    tx_id: &str,
    seq: u64,
    parent: Option<String>,
    ops: Vec<Op>,
    bindings: &[(&str, &str)],
    committed_at: i64,
) -> String {
    let intent = Tx {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: seq - 1,
        schema_version: SCHEMA_VERSION,
        ops: ops.clone(),
        args_hash: None,
    };
    let bindings: std::collections::BTreeMap<String, String> = bindings
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let resolved = fin_core::resolve_ops(&ops, &bindings).unwrap();
    let rec = Record::new_edit(
        tx_id.into(),
        seq,
        SCHEMA_VERSION,
        parent,
        "test".into(),
        committed_at,
        intent,
        resolved,
        vec![],
        bindings,
    );
    let bytes = encode_record(&rec);
    let hash = format!("blake3:{}", blake3::hash(&bytes).to_hex());
    db(s)
        .execute(
            "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                 added_at, seen_at, current)
         VALUES (?1, 'main', 'local', 'DIVERGED_TAIL', 'crafted', ?2, 0, 0, 1)",
            params![hash, bytes],
        )
        .unwrap();
    hash
}

#[test]
fn literal_id_collision_is_reported_and_group_not_created() {
    let e = env();
    let Split { mut b, .. } = split(&e, |_| {}, |_| {}, winner);
    // A record forking at t0 that names B's c1, which no record on its lineage made.
    let t0 = tx_hash(&b, "t0");
    craft(&b, "x9", 2, Some(t0), vec![trim("c1")], &[], 1);
    let groups = b.recover_groups().unwrap();
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert!(!g.recoverable);
    assert_eq!(g.problems.len(), 1);
    let p = &g.problems[0];
    assert_eq!(
        (p.reason.as_str(), p.tx_id.as_str()),
        ("ID_COLLISION", "x9")
    );
    assert_eq!(p.details.as_deref().unwrap()["id"], "c1");

    let r = create_all(&mut b, "rec");
    assert!(r.created.is_empty() && r.committed.is_empty());
    let err = fin_core::CoreError::from(
        b.recover(req("rec2", Some(RecoverSel::Group(g.id.clone()))))
            .unwrap_err(),
    );
    assert_eq!(err.code, "INVALID_OP");
    assert_eq!(err.details["problems"][0]["reason"], "ID_COLLISION");
    assert_eq!(b.branches(true).unwrap().len(), 1, "nothing created");
    assert_eq!(counted(&b), 1, "still counted");
}

#[test]
fn group_without_common_history_is_listed_not_created() {
    let e = env();
    let Split { mut b, .. } = split(&e, |_| {}, |_| {}, winner);
    let unknown = format!("blake3:{}", "0".repeat(64));
    craft(
        &b,
        "x9",
        7,
        Some(unknown),
        vec![asset("/m/9.mov")],
        &[("$a", "a9")],
        1,
    );
    let g = b.recover_groups().unwrap();
    assert_eq!(g.len(), 1);
    assert!(!g[0].recoverable);
    assert_eq!(g[0].problems[0].reason, "NO_COMMON_HISTORY");
    let r = create_all(&mut b, "rec");
    assert!(r.created.is_empty());
    assert_eq!(counted(&b), 1);
    let err = fin_core::CoreError::from(
        b.recover(req("rec2", Some(RecoverSel::Group("000000000000".into()))))
            .unwrap_err(),
    );
    assert_eq!(
        (err.code.as_str(), &err.details["kind"]),
        ("NOT_FOUND", &serde_json::json!("group"))
    );
}

/// One chain that forks inside quarantine (two children of one record) and one on its own:
/// three groups, the two children forking from the copy of their parent.
#[test]
fn groups_split_by_parent_chain() {
    let e = env();
    let Split { mut b, .. } = split(&e, |_| {}, |_| {}, winner);
    let t0 = tx_hash(&b, "t0");
    let u1 = tx_hash(&b, "u1");
    let r1 = craft(
        &b,
        "r1",
        2,
        Some(t0),
        vec![insert(0, "a1")],
        &[("$c", "c50")],
        10,
    );
    craft(&b, "r2a", 3, Some(r1.clone()), vec![trim("c50")], &[], 20);
    craft(
        &b,
        "r2b",
        3,
        Some(r1),
        vec![insert(1, "a1")],
        &[("$c", "c51")],
        30,
    );
    craft(
        &b,
        "s1",
        3,
        Some(u1),
        vec![asset("/m/s.mov")],
        &[("$a", "a50")],
        40,
    );
    let groups = b.recover_groups().unwrap();
    let firsts: Vec<(&str, usize, usize)> = groups
        .iter()
        .map(|g| {
            (
                g.problems.first().map_or("", |p| p.reason.as_str()),
                g.records,
                g.children.len(),
            )
        })
        .collect();
    assert_eq!(groups.len(), 4, "{groups:#?}");
    assert_eq!(
        firsts.iter().filter(|f| f.0.is_empty()).count(),
        4,
        "{groups:#?}"
    );
    let parent = groups.iter().find(|g| g.children.len() == 2).unwrap();
    assert_eq!((parent.records, parent.fork_seq), (1, 1));
    let alone = groups.iter().find(|g| g.fork_seq == 2).unwrap();
    assert!(alone.children.is_empty());

    let r = create_all(&mut b, "rec");
    assert_eq!(r.created.len(), 4);
    let by_parent = |p: &str| {
        r.created
            .iter()
            .filter(|c| c.parent.as_deref() == Some(p))
            .count()
    };
    let root = r.created.iter().find(|c| c.fork_seq == 1).unwrap();
    assert_eq!(by_parent(MAIN), 2);
    assert_eq!(
        by_parent(&root.id),
        2,
        "both children fork from the copy of r1"
    );
    for c in &r.created {
        let (_, p) = b.snapshot(&c.id).unwrap();
        assert!(
            !clips(&p).contains(&"c50".to_string()),
            "fresh ids: {:?}",
            clips(&p)
        );
    }
    assert_eq!(counted(&b), 0);
    assert!(b.recover_groups().unwrap().is_empty());
    assert_verifies(&b);
}

#[test]
fn recover_names_unique_within_one_second() {
    let e = env();
    let Split { mut b, .. } = split(&e, |_| {}, |_| {}, winner);
    let t0 = tx_hash(&b, "t0");
    let u1 = tx_hash(&b, "u1");
    let at = 1_793_923_200_123; // 2026-11-06 00:00:00.123 UTC
    craft(
        &b,
        "p1",
        2,
        Some(t0),
        vec![asset("/m/p.mov")],
        &[("$a", "a60")],
        at,
    );
    craft(
        &b,
        "q1",
        3,
        Some(u1),
        vec![asset("/m/q.mov")],
        &[("$a", "a61")],
        at + 500,
    );
    let base = format!("recovered-{}-20261106-000000", &b.server_id()[..8]);
    branch(&mut b, "taken", &base, MAIN);
    let names: Vec<String> = b
        .recover_groups()
        .unwrap()
        .into_iter()
        .map(|g| g.name)
        .collect();
    assert_eq!(names, [format!("{base}-2"), format!("{base}-3")]);
    let r = create_all(&mut b, "rec");
    let made: Vec<&str> = r.created.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(made, names);
}

/// F applied A's late line and rewound it (`SUPERSEDED`); B never applied it
/// (`STALE_EPOCH`). The same group on both; F plans it (a dry run on a follower), B makes
/// it, and F follows: the branch, and its marks.
#[test]
fn superseded_rows_after_rewind_become_branch() {
    let e = env();
    let Split {
        mut a,
        mut b,
        mut f,
    } = split(
        &e,
        |_| {},
        |a| {
            on(a, "x1", MAIN, vec![insert(0, "a1")]);
        },
        winner,
    );
    e.poll(&mut f);
    assert_eq!(quarantine(&f), [("x1".into(), "SUPERSEDED".into())]);
    e.poll(&mut b);
    assert_eq!(quarantine(&b), [("x1".into(), "STALE_EPOCH".into())]);
    let gf = f.recover_groups().unwrap();
    let gb = b.recover_groups().unwrap();
    assert_eq!(gf.len(), 1);
    assert_eq!(
        (
            gf[0].id.as_str(),
            gf[0].server.as_str(),
            gf[0].origin.as_str()
        ),
        (gb[0].id.as_str(), a.server_id(), "folder")
    );
    assert_eq!(gf[0].reasons.get("SUPERSEDED"), Some(&1));
    assert_name(&gf[0].name, a.server_id());

    // A dry run on the follower plans it and keeps nothing.
    let mut dry = req("dry", Some(RecoverSel::All));
    dry.dry_run = true;
    let r = f.recover(dry).unwrap();
    assert!(r.dry_run);
    assert_eq!(r.created.len(), 1);
    assert_eq!(kinds(&r.committed), ["branch", "cherryPick"]);
    assert_eq!(f.branches(true).unwrap().len(), 1);
    assert_eq!(counted(&f), 1);
    assert_eq!(f.recover_groups().unwrap(), gf);
    // A real one on the follower is refused.
    let err = fin_core::CoreError::from(f.recover(req("real", Some(RecoverSel::All))).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY");

    let r = create_all(&mut b, "rec");
    assert_eq!(r.created.len(), 1);
    assert_eq!(counted(&b), 0);
    e.flush(&mut b);
    e.poll(&mut f);
    let id = &r.created[0].id;
    assert_eq!(f.snapshot(id).unwrap(), b.snapshot(id).unwrap());
    assert_eq!(counted(&f), 0, "the marks follow");
    assert!(f.recover_groups().unwrap().is_empty());
    assert_verifies(&b);
    assert_verifies(&f);
    // A, back as a follower, applies B's line and sees the same.
    a.set_test_lease(1, false);
    e.poll(&mut a);
    assert_eq!(a.snapshot(id).unwrap(), b.snapshot(id).unwrap());
    assert_eq!(counted(&a), 0);
}

/// Decision 8: A's late line made `br1` and wrote on it; the line lost, so `br1` is set
/// aside whole. Its records are a child group, made after their parent group, forked from
/// the copy of its fork record.
#[test]
fn set_aside_child_branch_recovers_as_child_group() {
    let e = env();
    let Split { mut b, mut f, .. } = split(
        &e,
        |_| {},
        |a| {
            on(a, "x0", MAIN, vec![insert(0, "a1")]);
            assert_eq!(branch(a, "xb", "side", MAIN), "br1");
            on(a, "x1", "br1", vec![insert(1, "a1")]);
        },
        winner,
    );
    e.poll(&mut f);
    e.poll(&mut b);
    let q = quarantine(&b);
    assert_eq!(q.len(), 3, "{q:?}");
    let groups = b.recover_groups().unwrap();
    assert_eq!(groups.len(), 2, "{groups:#?}");
    let (parent, child) = (&groups[0], &groups[1]);
    assert_eq!(
        (
            parent.records_branch.as_str(),
            parent.branch.as_str(),
            parent.records
        ),
        (MAIN, MAIN, 2)
    );
    assert_eq!((child.records_branch.as_str(), child.records), ("br1", 1));
    // It forks from the branch its parent group makes, at the copy of x0.
    assert_eq!(
        (
            child.branch.as_str(),
            child.fork_seq,
            child.fork_group.as_deref()
        ),
        (parent.name.as_str(), 2, Some(parent.id.as_str()))
    );
    assert_eq!(parent.children, std::slice::from_ref(&child.id));
    assert_eq!(
        f.recover_groups().unwrap().len(),
        2,
        "the follower lists the same"
    );

    // The child alone cannot be made before its parent; the parent brings its children.
    let err = fin_core::CoreError::from(
        b.recover(req("one", Some(RecoverSel::Group(child.id.clone()))))
            .unwrap_err(),
    );
    assert_eq!(err.code, "INVALID_OP");
    assert_eq!(
        err.fix.unwrap(),
        format!("fin project recover --create {}", parent.id)
    );
    let r = b
        .recover(req("rec", Some(RecoverSel::Group(parent.id.clone()))))
        .unwrap();
    assert_eq!(r.created.len(), 2);
    let (p, c) = (&r.created[0], &r.created[1]);
    assert_eq!((p.parent.as_deref(), p.fork_seq), (Some(MAIN), 1));
    assert_eq!((c.parent.as_deref(), c.fork_seq), (Some(p.id.as_str()), 2));
    // Each name is `recovered-<server>-<stamp>`, stamped with its group's first commit second:
    // in the same second the child takes `-2`, a second later a later stamp of its own.
    let stamp = "YYYYMMDD-HHMMSS".len();
    let server = &p.name[..p.name.len() - stamp];
    if c.name.starts_with(&p.name) {
        assert_eq!(c.name, format!("{}-2", p.name), "{:?}", (&p.name, &c.name));
    } else {
        assert!(
            c.name.starts_with(server) && c.name.len() == p.name.len() && c.name > p.name,
            "{:?}",
            (&p.name, &c.name)
        );
    }
    assert_eq!(
        kinds(&r.committed),
        ["branch", "cherryPick", "branch", "cherryPick"]
    );
    let (_, state) = b.snapshot(&c.id).unwrap();
    assert_eq!(clips(&state).len(), 2, "x0 and x1, under fresh ids");
    assert_eq!(
        counted(&b),
        0,
        "the set-aside branch record is recovered with them"
    );
    assert!(b.recover_groups().unwrap().is_empty());
    e.flush(&mut b);
    e.poll(&mut f);
    assert_eq!(counted(&f), 0);
    assert_eq!(f.snapshot(&c.id).unwrap(), b.snapshot(&c.id).unwrap());
    assert_verifies(&f);
}

#[test]
fn recovered_marks_rebuild_from_log() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
    });
    create_all(&mut a, "rec");
    e.flush(&mut a);
    assert_eq!(counted(&a), 0);
    let before = marks(&a);
    assert_eq!(before.len(), 1);
    let rep = a.rebuild().unwrap();
    assert!(rep.verify.ok, "{:#?}", rep.verify);
    assert_eq!(quarantine(&a).len(), 1, "the row came along");
    e.poll(&mut a);
    assert_eq!(
        marks(&a),
        before,
        "the side table is read again from the rows"
    );
    assert_eq!(counted(&a), 0, "still not counted");
    assert!(a.recover_groups().unwrap().is_empty());
    // A server with no database rebuilds the same marks from the folder.
    let g = e.open("g");
    assert!(g.recover_groups().unwrap().is_empty());
    assert_eq!(g.branches(true).unwrap(), a.branches(true).unwrap());
}

#[test]
fn recovered_marks_follow_to_followers() {
    let e = env();
    let Split { mut b, mut f, .. } = split(
        &e,
        |_| {},
        |a| {
            on(a, "x1", MAIN, vec![insert(0, "a1")]);
            on(a, "x2", MAIN, vec![trim("c1")]);
        },
        winner,
    );
    e.poll(&mut f);
    e.poll(&mut b);
    assert_eq!(counted(&f), 2);
    let mut g = e.open("g");
    assert_eq!(counted(&g), 2, "a fresh follower sets them aside too");
    create_all(&mut b, "rec");
    e.flush(&mut b);
    for s in [&mut f, &mut g] {
        e.poll(s);
        assert_eq!(counted(s), 0);
        assert!(s.recover_groups().unwrap().is_empty());
        assert_eq!(quarantine(s).len(), 2, "kept");
        assert_eq!(marks(s), marks(&b));
        assert_verifies(s);
    }
}

/// `quarantine_marks`, sorted.
fn marks(s: &Store) -> Vec<(String, String, Option<String>)> {
    let c = db(s);
    let mut stmt = c
        .prepare("SELECT tx_hash, kind, target FROM quarantine_marks ORDER BY tx_hash")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A `CHAIN_FORK` (two servers at one epoch), resolved by the manual procedure: one side's
/// files moved out of the folder, then `--import` reads them into quarantine and
/// `--create all` makes them a branch.
#[test]
fn import_dir_reads_moved_segments_into_quarantine() {
    let e = env();
    let a = e.create("a");
    let mut c = e.open("c");
    let mut d = e.open("d");
    let mut f = e.open("f");
    c.set_test_lease(1, true);
    commit(&mut c, "x", vec![asset("/m/x.mov")]);
    commit(&mut c, "x2", vec![insert(0, "a1")]);
    e.flush(&mut c);
    let x_files = all_segments(&e.dir);
    assert!(!x_files.is_empty());
    d.set_test_lease(1, true);
    commit(&mut d, "z", vec![asset("/m/z.mov")]);
    d.close().unwrap();
    e.poll(&mut f);
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    let r = f.recover(req("list", None)).unwrap();
    assert!(r.manual.as_deref().unwrap().contains("--import"), "{r:?}");

    // The manual procedure: C's side moved out of the folder.
    let moved = e.root.join("moved");
    fs::create_dir_all(&moved).unwrap();
    for p in &x_files {
        fs::rename(p, moved.join(p.file_name().unwrap())).unwrap();
    }
    e.poll(&mut f);
    assert_eq!(
        f.read_only().filter(|r| *r == ReadOnlyReason::ChainFork),
        None
    );

    // A dry run imports and plans, and keeps nothing.
    let mut dry = req("imp", Some(RecoverSel::All));
    dry.import_dir = Some(moved.clone());
    dry.dry_run = true;
    let r = f.recover(dry).unwrap();
    assert_eq!((r.imported, r.created.len()), (2, 1));
    assert!(quarantine(&f).is_empty());

    // Import alone works while read-only (F follows: LEASE_HELD).
    assert!(f.read_only().is_some());
    let mut imp = req("imp", None);
    imp.import_dir = Some(moved.clone());
    let r = f.recover(imp).unwrap();
    assert_eq!(r.imported, 2);
    assert_eq!(
        quarantine(&f),
        [
            ("x".into(), "IMPORTED".into()),
            ("x2".into(), "IMPORTED".into())
        ]
    );
    assert_eq!(r.groups.len(), 1);
    let g = &r.groups[0];
    assert_eq!(
        (g.origin.as_str(), g.server.as_str(), g.fork_seq),
        ("foreign", c.server_id(), 0)
    );
    // Imported rows stay set aside on later polls.
    e.poll(&mut f);
    assert_eq!(counted(&f), 2);

    f.set_test_lease(1, true);
    let r = create_all(&mut f, "rec");
    assert_eq!(r.created.len(), 1);
    let (_, p) = f.snapshot(&r.created[0].id).unwrap();
    let paths: Vec<&str> = p.assets.values().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, ["/m/x.mov"], "the branch holds C's side");
    assert!(
        !p.assets.contains_key("a1"),
        "D's a1 is not reused: {:?}",
        p.assets.keys()
    );
    assert_eq!(clips(&p).len(), 1);
    assert_eq!(counted(&f), 0);
    assert_ne!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    assert_verifies(&f);
    drop(a);
}

#[test]
fn recover_retry_same_tx_id_is_idempotent() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
    });
    let first = create_all(&mut a, "rec");
    let again = create_all(&mut a, "rec");
    assert_eq!(again.created, first.created);
    assert_eq!(again.committed, first.committed);
    assert!(!again.dry_run);
    assert_eq!(a.branches(true).unwrap().len(), 2);
    // Another txId finds nothing left to recover and writes nothing.
    let other = create_all(&mut a, "rec-b");
    assert!(other.created.is_empty() && other.committed.is_empty() && other.groups.is_empty());
    // A txId some other record holds is refused.
    on(&mut a, "k-1", MAIN, vec![asset("/m/k.mov")]);
    let err = fin_core::CoreError::from(a.recover(req("k", Some(RecoverSel::All))).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    let err = fin_core::CoreError::from(a.recover(req(" ", Some(RecoverSel::All))).unwrap_err());
    assert_eq!(err.code, "INVALID_OP");
}

/// An undo in the tail names its unit's clip literally (its ops come from the inverse): the
/// copy names the fresh id.
#[test]
fn undo_in_the_tail_names_the_fresh_id() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
        on(a, "x2", MAIN, vec![insert(1, "a1")]);
        a.undo(HistoryReq {
            tx_id: "x3".into(),
            author: "test".into(),
            branch: MAIN.into(),
            dry_run: false,
            args_hash: None,
            to: None,
            after_seq: None,
        })
        .unwrap();
    });
    assert_eq!(quarantine(&a).len(), 3);
    let r = create_all(&mut a, "rec");
    let fresh = r.committed[2].bindings["$c"].clone();
    assert_ne!(fresh, "c2");
    assert_eq!(r.committed[3].tx.ops, [remove_item(&fresh)]);
    let (_, p) = a.snapshot(&r.created[0].id).unwrap();
    assert_eq!(clips(&p), [r.committed[1].bindings["$c"].clone()]);
    assert_verifies(&a);
}

// Review of Task 16: what stands for records without ops, and merges in a tail.

/// As [`diverged`], with `before` committed and exported by A first.
fn diverged_with(e: &Env, before: impl FnOnce(&mut Store), tail: impl FnOnce(&mut Store)) -> Split {
    let mut s = split(e, before, |_| {}, winner);
    tail(&mut s.a);
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    s
}

fn rename(s: &mut Store, tx: &str, b: &str, name: &str) {
    s.set_branch(fin_store::BranchSet {
        tx_id: tx.into(),
        author: "test".into(),
        branch: b.into(),
        name: Some(name.into()),
        archived: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
}

fn merge(s: &mut Store, tx: &str, from: &str, into: &str) -> fin_store::MergeReport {
    s.merge(fin_store::MergeReq {
        tx_id: tx.into(),
        author: "test".into(),
        from: from.into(),
        into: into.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
        include: None,
        source_up_to: None,
    })
    .unwrap()
}

fn name_of(s: &Store, id: &str) -> String {
    s.branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap()
        .name
}

/// The `recovers` of a record without ops (`checkpoint`, `branch`, `branchSet`).
fn stands_for(c: &CommittedTx) -> Option<&str> {
    match c.link.as_deref() {
        Some(
            Link::Checkpoint { recovers, .. }
            | Link::Branch { recovers, .. }
            | Link::BranchSet { recovers, .. },
        ) => recovers.as_deref(),
        _ => None,
    }
}

/// B1: a rename of a live branch in the tail is made again, not dropped.
#[test]
fn branch_set_of_live_branch_in_tail_is_applied_again() {
    let e = env();
    let Split { mut a, mut f, .. } = diverged_with(
        &e,
        |a| {
            branch(a, "mk", "side", MAIN);
        },
        |a| {
            on(a, "x1", MAIN, vec![insert(0, "a1")]);
            rename(a, "xr", "br1", "renamed");
            on(a, "x2", MAIN, vec![insert(1, "a1")]);
        },
    );
    assert_eq!(counted(&a), 3);
    let r = create_all(&mut a, "rec");
    assert_eq!(
        kinds(&r.committed),
        ["branch", "cherryPick", "other", "cherryPick"]
    );
    assert_eq!(
        stands_for(&r.committed[2]),
        Some(hash_of_quarantined(&a, "xr").as_str())
    );
    assert_eq!(name_of(&a, "br1"), "renamed");
    assert_eq!(counted(&a), 0);
    assert!(a.recover_groups().unwrap().is_empty());
    assert_verifies(&a);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(name_of(&f, "br1"), "renamed");
}

/// B1: a tail of only a rename makes no branch; the rename is made again.
#[test]
fn lone_branch_set_tail_is_applied_again() {
    let e = env();
    let Split { mut a, .. } = diverged_with(
        &e,
        |a| {
            branch(a, "mk", "side", MAIN);
        },
        |a| rename(a, "xr", "br1", "renamed"),
    );
    let g = a.recover_groups().unwrap();
    assert_eq!(g.len(), 1);
    assert!(g[0].recoverable && g[0].name.is_empty(), "{g:#?}");
    let r = create_all(&mut a, "rec");
    assert!(r.created.is_empty());
    assert_eq!(r.committed.len(), 1);
    assert_eq!(name_of(&a, "br1"), "renamed");
    assert_eq!(counted(&a), 0);
    // A retry answers from the log.
    let again = create_all(&mut a, "rec");
    assert_eq!(again.committed, r.committed);
}

/// B1: a rename whose name is taken now stays in quarantine with a fix, and is made once
/// the name is free.
#[test]
fn branch_set_whose_name_is_taken_waits_then_applies() {
    let e = env();
    let mut s = split(
        &e,
        |a| {
            branch(a, "mk", "side", MAIN);
        },
        |_| {},
        |b| {
            branch(b, "mk2", "other", MAIN);
        },
    );
    on(&mut s.a, "x1", MAIN, vec![insert(0, "a1")]);
    rename(&mut s.a, "xr", "br1", "other");
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    let mut a = s.a;
    let g = a.recover_groups().unwrap();
    assert_eq!(g.len(), 1);
    assert!(g[0].recoverable);
    assert_eq!(g[0].notes[0].reason, "NAME_TAKEN", "{g:#?}");
    assert!(
        g[0].notes[0].fix.contains("fin branch rename br2"),
        "{}",
        g[0].notes[0].fix
    );
    create_all(&mut a, "rec");
    assert_eq!(name_of(&a, "br1"), "side");
    assert_eq!(counted(&a), 1, "the rename is still set aside");
    let left = a.recover_groups().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].recoverable, left[0].problems[0].reason.as_str()),
        (false, "NAME_TAKEN")
    );
    rename(&mut a, "free", "br2", "elsewhere");
    let r = create_all(&mut a, "rec2");
    assert_eq!(r.committed.len(), 1);
    assert_eq!(name_of(&a, "br1"), "other");
    assert_eq!(counted(&a), 0);
    assert_verifies(&a);
}

/// B1: an empty branch made in the tail is made again on the recovered branch.
#[test]
fn empty_branch_in_tail_is_made_again() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
        branch(a, "xb", "feature", MAIN);
        on(a, "x2", MAIN, vec![insert(1, "a1")]);
    });
    let r = create_all(&mut a, "rec");
    assert_eq!(
        kinds(&r.committed),
        ["branch", "cherryPick", "branch", "cherryPick"]
    );
    assert_eq!(r.created.len(), 2);
    let (rec, feature) = (&r.created[0], &r.created[1]);
    assert_eq!(feature.name, "feature");
    assert_eq!(
        (feature.parent.as_deref(), feature.fork_seq),
        (Some(rec.id.as_str()), 2),
        "forked at the copy of x1"
    );
    assert_eq!(
        stands_for(&r.committed[2]),
        Some(hash_of_quarantined(&a, "xb").as_str())
    );
    let (_, p) = a.snapshot(&feature.id).unwrap();
    assert_eq!(clips(&p).len(), 1);
    assert_eq!(counted(&a), 0);
    assert_verifies(&a);
}

/// B1: a rename of a branch set aside with it comes back with that branch.
#[test]
fn branch_set_of_set_aside_branch_comes_back_with_it() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        branch(a, "xb", "feat", MAIN);
        on(a, "x1", "br1", vec![insert(0, "a1")]);
        rename(a, "xr", "br1", "feat2");
    });
    assert_eq!(counted(&a), 3);
    let g = a.recover_groups().unwrap();
    assert_eq!(g.len(), 2, "{g:#?}");
    let r = create_all(&mut a, "rec");
    assert_eq!(kinds(&r.committed), ["branch", "cherryPick"]);
    assert_eq!(
        stands_for(&r.committed[0]),
        Some(hash_of_quarantined(&a, "xb").as_str())
    );
    assert_eq!(counted(&a), 0);
    assert!(a.recover_groups().unwrap().is_empty());
}

/// B2: a merge in the tail is copied as its source's root with the source's ids, so merging
/// the source again brings nothing twice.
#[test]
fn merge_in_tail_keeps_its_source_root_and_ids() {
    let e = env();
    let Split { mut a, .. } = diverged_with(
        &e,
        |a| {
            let b = branch(a, "mk", "side", MAIN);
            on(a, "s1", &b, vec![insert(0, "a1")]);
        },
        |a| {
            merge(a, "xm", "br1", MAIN);
        },
    );
    let r = create_all(&mut a, "rec");
    let made = r.created[0].id.clone();
    let copy = &r.committed[1];
    match copy.link.as_deref() {
        Some(Link::CherryPick {
            origin,
            from,
            origin_hash,
            recovers,
        }) => {
            assert_eq!((origin.as_str(), from.as_str()), ("s1", "br1"));
            assert_eq!(
                origin_hash.as_deref(),
                Some(tx_hash(&a, "s1").as_str()),
                "its root"
            );
            assert_eq!(
                recovers.as_deref(),
                Some(hash_of_quarantined(&a, "xm-1").as_str())
            );
        }
        other => panic!("{other:?}"),
    }
    let s1 = a.ops_since("br1", 0).unwrap();
    let s1 = s1.iter().find(|c| c.tx.tx_id == "s1").unwrap();
    assert_eq!(copy.bindings, s1.bindings, "the source's ids");
    assert!(r.groups[0].notes.is_empty());
    let again = merge(&mut a, "m2", "br1", &made);
    assert!(again.committed.is_empty(), "{again:?}");
    let (_, p) = a.snapshot(&made).unwrap();
    assert_eq!(clips(&p), clips(&a.snapshot("br1").unwrap().1));
    assert_verifies(&a);
}

/// B2: the recovered branch merges back into a `main` that merged the source itself.
#[test]
fn recovered_merge_copy_merges_back_without_duplicates() {
    let e = env();
    let mut s = split(
        &e,
        |a| {
            let b = branch(a, "mk", "side", MAIN);
            on(a, "s1", &b, vec![insert(0, "a1")]);
        },
        |_| {},
        |b| {
            merge(b, "bm", "br1", MAIN);
        },
    );
    merge(&mut s.a, "xm", "br1", MAIN);
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    let mut a = s.a;
    let r = create_all(&mut a, "rec");
    let made = r.created[0].id.clone();
    let before = clips(&a.snapshot(MAIN).unwrap().1);
    let back = merge(&mut a, "back", &made, MAIN);
    assert!(back.committed.is_empty(), "{back:?}");
    assert_eq!(clips(&a.snapshot(MAIN).unwrap().1), before);
}

/// B2: a set-aside branch and a merge of it within one tail stay one entity on their
/// recovered branches.
#[test]
fn set_aside_branch_and_its_merge_share_ids() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        branch(a, "xb", "feat", MAIN);
        on(a, "s2", "br1", vec![insert(0, "a1")]);
        merge(a, "xm", "br1", MAIN);
    });
    let r = create_all(&mut a, "rec");
    assert_eq!(r.created.len(), 2, "{:?}", r.created);
    let ids: Vec<(&str, &std::collections::BTreeMap<String, String>)> = r
        .committed
        .iter()
        .filter(|c| matches!(c.link.as_deref(), Some(Link::CherryPick { .. })))
        .map(|c| match c.link.as_deref() {
            Some(Link::CherryPick { origin, .. }) => (origin.as_str(), &c.bindings),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].0, "s2");
    assert_eq!(ids[1].0, "s2");
    assert_eq!(ids[0].1, ids[1].1, "one entity");
    assert_ne!(ids[0].1["$c"], "c1");
    let (main_copy, branch_copy) = (&r.created[0], &r.created[1]);
    let m = merge(&mut a, "m3", &branch_copy.id, &main_copy.id);
    assert!(m.committed.is_empty(), "{m:?}");
    assert_eq!(counted(&a), 0);
    assert_verifies(&a);
}

/// S1, S2: `--import` lists the files it did not read, and counts only rows it added or
/// made current again.
#[test]
fn import_reports_skipped_files_and_counts_new_rows() {
    let e = env();
    let a = e.create("a");
    let mut c = e.open("c");
    let mut d = e.open("d");
    let mut f = e.open("f");
    c.set_test_lease(1, true);
    commit(&mut c, "x", vec![asset("/m/x.mov")]);
    commit(&mut c, "x2", vec![insert(0, "a1")]);
    e.flush(&mut c);
    let x_files = all_segments(&e.dir);
    d.set_test_lease(1, true);
    commit(&mut d, "z", vec![asset("/m/z.mov")]);
    d.close().unwrap();
    e.poll(&mut f);
    let moved = e.root.join("moved");
    fs::create_dir_all(moved.join("dup")).unwrap();
    for p in &x_files {
        fs::copy(p, moved.join("dup").join(p.file_name().unwrap())).unwrap();
        fs::rename(p, moved.join(p.file_name().unwrap())).unwrap();
    }
    fs::write(moved.join("junk.fseg"), b"not a segment").unwrap();
    let bytes = fs::read(moved.join(x_files[0].file_name().unwrap())).unwrap();
    fs::write(moved.join("trunc.fseg"), &bytes[..bytes.len() / 2]).unwrap();
    e.poll(&mut f);
    let mut imp = req("imp", None);
    imp.import_dir = Some(moved.clone());
    let r = f.recover(imp.clone()).unwrap();
    assert_eq!(r.imported, 2, "the duplicate file counts once");
    let mut skipped: Vec<String> = r
        .skipped_files
        .iter()
        .map(|s| {
            assert!(s.reason.starts_with("not a segment"), "{s:?}");
            Path::new(&s.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    skipped.sort();
    assert_eq!(skipped, ["junk.fseg", "trunc.fseg"]);
    let r = f.recover(imp).unwrap();
    assert_eq!(r.imported, 0, "nothing new the second time");
    assert_eq!(counted(&f), 2);
    let mut missing = req("imp", None);
    missing.import_dir = Some(moved.join("nowhere"));
    let err = fin_core::CoreError::from(f.recover(missing).unwrap_err());
    assert_eq!(err.code, "IO_ERROR");
    drop(a);
}

/// S3: a group that can never be recovered can be dismissed on this server; a recoverable
/// one cannot.
#[test]
fn unrecoverable_group_can_be_dismissed() {
    let e = env();
    let Split { mut b, .. } = split(&e, |_| {}, |_| {}, winner);
    let unknown = format!("blake3:{}", "0".repeat(64));
    craft(
        &b,
        "x9",
        7,
        Some(unknown),
        vec![asset("/m/9.mov")],
        &[("$a", "a9")],
        1,
    );
    let t0 = tx_hash(&b, "t0");
    craft(
        &b,
        "y1",
        2,
        Some(t0),
        vec![asset("/m/y.mov")],
        &[("$a", "a70")],
        2,
    );
    let groups = b.recover_groups().unwrap();
    let bad = groups.iter().find(|g| !g.recoverable).unwrap();
    let good = groups.iter().find(|g| g.recoverable).unwrap();
    assert!(
        bad.problems[0].fix.contains("dismiss"),
        "{}",
        bad.problems[0].fix
    );
    assert_eq!(counted(&b), 2);
    let mut no = req("d", None);
    no.dismiss = Some(good.id.clone());
    let err = fin_core::CoreError::from(b.recover(no).unwrap_err());
    assert_eq!(err.code, "INVALID_OP");
    let mut yes = req("d", None);
    yes.dismiss = Some(bad.id.clone());
    yes.dry_run = true;
    assert_eq!(b.recover(yes.clone()).unwrap().dismissed, 1);
    assert_eq!(counted(&b), 2, "a dry run keeps nothing");
    yes.dry_run = false;
    let r = b.recover(yes).unwrap();
    assert_eq!(r.dismissed, 1);
    assert_eq!(r.groups.len(), 1);
    assert_eq!(counted(&b), 1);
    assert!(!b.status().warnings.iter().any(|w| w.message.contains("x9")));
}

/// Verify compares the marks' side table with the rows' bytes.
#[test]
fn verify_checks_quarantine_marks() {
    let e = env();
    let Split { a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
    });
    assert_eq!(marks(&a).len(), 1);
    assert_verifies(&a);
    db(&a)
        .execute("UPDATE quarantine_marks SET kind = 'branch'", [])
        .unwrap();
    let v = a.verify();
    let c = v.checks.iter().find(|c| c.name == "branch_index").unwrap();
    assert!(
        !c.ok && c.details["marks"].as_array().unwrap().len() == 1,
        "{c:#?}"
    );
}

/// Plan 4C review B1: a tail that edits an effect it added comes back on the effect's new id
/// (the effect segment of a parameter path is remapped like any other id).
#[test]
fn recovered_tail_follows_the_effect_it_added() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
        let fx = fin_core::EffectAdd {
            item: "c1".into(),
            id: "$fx".into(),
            kind: "blur".into(),
            at: None,
            params: Default::default(),
        };
        let x2 = on(a, "x2", MAIN, vec![fx.into()]);
        let path = format!("effects.{}.radius", x2.bindings["$fx"]);
        let value = Some(fin_core::ParamValue::Number(5.0));
        let set = fin_core::ParamSet {
            item: "c1".into(),
            path,
            value,
        };
        on(a, "x3", MAIN, vec![set.into()]);
    });
    let groups = a.recover_groups().unwrap();
    assert!(groups.iter().all(|g| g.recoverable), "{groups:#?}");
    let r = create_all(&mut a, "rec");
    // `committed[0]` is the recovered branch's `branch` record.
    let Op::ParamSet(set) = &r.committed[3].tx.ops[0] else {
        panic!("{:?}", r.committed)
    };
    let (c, fx) = (
        &r.committed[1].bindings["$c"],
        &r.committed[2].bindings["$fx"],
    );
    assert!(c != "c1" && fx != "fx1", "fresh ids: {c} {fx}");
    assert_eq!(
        (set.item.as_str(), set.path.clone()),
        (c.as_str(), format!("effects.{fx}.radius"))
    );
}
