//! The in-transaction write guard (sub-project 5A Decision 17, spec §4.4 stage 6): every
//! write path checks the branch it changes, by the id the store resolves inside the write
//! transaction, against `Store::with_guard`'s guard. A refusal (`BRANCH_NOT_ALLOWED`,
//! `BRANCH_PROTECTED`) writes nothing and leaves the txId free for a retry that may write.

mod integration_support;

use fin_core::{CommittedTx, CoreError, Op, SCHEMA_VERSION, Tx};
use fin_store::{
    BranchCreate, BranchSet, CheckpointReq, CherryPickReq, HistoryReq, MAIN, MergeReq, RecoverReq,
    RecoverSel, RevertReq, Store, StoreError, WriteGuard,
};
use integration_support::diverge::*;
use integration_support::*;
use rusqlite::Connection;

fn allow(ids: &[&str]) -> WriteGuard {
    WriteGuard {
        deny_branches: vec![],
        allow_branches: Some(ids.iter().map(|s| s.to_string()).collect()),
    }
}

fn deny(ids: &[&str]) -> WriteGuard {
    WriteGuard {
        deny_branches: ids.iter().map(|s| s.to_string()).collect(),
        allow_branches: None,
    }
}

/// What a write may change: the number of records, and every branch's name, state and head.
#[derive(Debug, PartialEq)]
struct Snap {
    txs: i64,
    branches: Vec<(String, String, String, u64)>,
}

fn snap(s: &Store) -> Snap {
    let txs = Connection::open(s.db_path())
        .unwrap()
        .query_row("SELECT count(*) FROM txs", [], |r| r.get(0))
        .unwrap();
    let branches = s
        .branches(true)
        .unwrap()
        .into_iter()
        .map(|b| (b.id, b.name, format!("{:?}", b.state), b.head))
        .collect();
    Snap { txs, branches }
}

fn tx_on(s: &Store, id: &str, branch: &str, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: s.head(branch),
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    let tx = tx_on(s, id, branch, ops);
    s.commit(tx).unwrap()
}

fn branch(s: &mut Store, tx_id: &str, name: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: MAIN.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn set(tx_id: &str, b: &str, name: Option<&str>, archived: Option<bool>) -> BranchSet {
    BranchSet {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: b.into(),
        name: name.map(str::to_string),
        archived,
        dry_run: false,
        args_hash: None,
    }
}

fn history(tx_id: &str, branch: &str, dry_run: bool) -> HistoryReq {
    HistoryReq {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: branch.into(),
        dry_run,
        args_hash: None,
        to: None,
    }
}

/// `main` with an asset and an edit; `feature` (br1) with an edit `f1` of its own; `other`
/// (br2) without it.
fn project(e: &Env) -> (Store, String, String) {
    let mut s = e.create("a");
    commit(&mut s, "t0", vec![asset("/m/0.mov")]);
    commit(&mut s, "t1", vec![insert(0, "a1")]);
    let feature = branch(&mut s, "b1", "feature");
    let other = branch(&mut s, "b2", "other");
    on(&mut s, "f1", &feature, vec![insert(5, "a1")]);
    assert_eq!((feature.as_str(), other.as_str()), ("br1", "br2"));
    (s, feature, other)
}

/// The refusal `op` answers under `guard`, which must write nothing.
fn refused<T: std::fmt::Debug>(
    s: &mut Store,
    guard: WriteGuard,
    op: &mut impl FnMut(&mut Store) -> Result<T, StoreError>,
) -> CoreError {
    let before = snap(s);
    let e = s.with_guard(guard, |s| op(s)).expect_err("refused");
    assert_eq!(snap(s), before, "a refused write writes nothing");
    CoreError::from(e)
}

/// `op` changes `target`: refused under an allow-list of every other branch
/// (`BRANCH_NOT_ALLOWED`) and under a deny of `target` (`BRANCH_PROTECTED`), each writing
/// nothing, with `details {branch, name}`; then written under an allow-list of `target`
/// alone, with the same txId (a refusal does not consume it).
fn guarded<T: std::fmt::Debug>(
    s: &mut Store,
    target: &str,
    mut op: impl FnMut(&mut Store) -> Result<T, StoreError>,
) -> T {
    let name = s
        .branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == target)
        .unwrap()
        .name;
    let others: Vec<String> = s
        .branches(true)
        .unwrap()
        .into_iter()
        .map(|b| b.id)
        .filter(|id| id != target)
        .collect();
    let others: Vec<&str> = others.iter().map(String::as_str).collect();
    let e = refused(s, allow(&others), &mut op);
    assert_eq!(e.code, "BRANCH_NOT_ALLOWED", "{e:?}");
    assert_eq!(e.details["branch"], target);
    assert_eq!(e.details["name"], name.as_str());
    let e = refused(s, deny(&[target]), &mut op);
    assert_eq!(e.code, "BRANCH_PROTECTED", "{e:?}");
    assert_eq!(e.details["branch"], target);
    let before = snap(s);
    let out = s.with_guard(allow(&[target]), |s| op(s)).unwrap();
    assert_ne!(snap(s), before, "the allowed write writes");
    out
}

#[test]
fn commit_is_guarded() {
    let e = env();
    let (mut s, feature, _) = project(&e);
    let rev = guarded(&mut s, MAIN, |s| {
        let tx = tx_on(s, "c1", MAIN, vec![insert(10, "a1")]);
        s.commit(tx)
    })
    .rev;
    assert_eq!(s.head(MAIN), rev);
    // A guard on one branch says nothing about another.
    let tx = tx_on(&s, "c2", &feature, vec![insert(10, "a1")]);
    s.with_guard(deny(&[MAIN]), |s| s.commit(tx)).unwrap();
}

#[test]
fn undo_and_redo_are_guarded() {
    let e = env();
    let (mut s, feature, _) = project(&e);
    let undone = guarded(&mut s, &feature, |s| s.undo(history("u1", &feature, false)));
    assert_eq!(undone.committed.len(), 1);
    let redone = guarded(&mut s, &feature, |s| s.redo(history("r1", &feature, false)));
    assert_eq!(redone.committed.len(), 1);
    // A dry run is refused the same way (it plans inside the same write).
    let e = refused(&mut s, deny(&[&feature]), &mut |s: &mut Store| {
        s.undo(history("u2", &feature, true))
    });
    assert_eq!(e.code, "BRANCH_PROTECTED");
}

#[test]
fn checkpoint_is_guarded() {
    let e = env();
    let (mut s, _, _) = project(&e);
    guarded(&mut s, MAIN, |s| {
        s.checkpoint(CheckpointReq {
            tx_id: "cp1".into(),
            author: "test".into(),
            branch: MAIN.into(),
            name: "mark".into(),
            dry_run: false,
            args_hash: None,
        })
    });
}

fn merge(from: &str, into: &str) -> MergeReq {
    MergeReq {
        tx_id: "m1".into(),
        author: "test".into(),
        from: from.into(),
        into: into.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
    }
}

#[test]
fn merge_guards_into_and_only_reads_from() {
    let e = env();
    let (mut s, feature, _) = project(&e);
    let report = guarded(&mut s, MAIN, |s| s.merge(merge(&feature, MAIN)));
    assert!(!report.committed.is_empty());
    // `from` is only read: a deny of it, or an allow-list without it, lets the merge through.
    let e2 = env();
    let (mut s, feature, _) = project(&e2);
    s.with_guard(deny(&[&feature]), |s| s.merge(merge(&feature, MAIN)))
        .unwrap();
    let e3 = env();
    let (mut s, feature, _) = project(&e3);
    s.with_guard(allow(&[MAIN]), |s| s.merge(merge(&feature, MAIN)))
        .unwrap();
}

#[test]
fn cherry_pick_and_revert_are_guarded() {
    let e = env();
    let (mut s, feature, other) = project(&e);
    let picked = guarded(&mut s, &other, |s| {
        s.cherry_pick(CherryPickReq {
            tx_id: "p1".into(),
            author: "test".into(),
            origin: "f1".into(),
            into: other.clone(),
            dry_run: false,
            args_hash: None,
        })
    });
    assert_eq!(picked.committed.len(), 1);
    s.merge(merge(&feature, MAIN)).unwrap();
    let reverted = guarded(&mut s, MAIN, |s| {
        s.revert(RevertReq {
            tx_id: "rv1".into(),
            author: "test".into(),
            merge_id: "m1".into(),
            branch: MAIN.into(),
            dry_run: false,
            args_hash: None,
        })
    });
    assert_eq!(reverted.committed.len(), 1);
}

/// A new branch is not a target: its `branch` record goes on the parent's log, which the
/// create only reads (the fork), and the branch did not exist when the guard was computed,
/// so no list can name it. A credential limited to its own branches must be able to make
/// them, from a protected parent too.
#[test]
fn branch_create_is_not_a_target() {
    let e = env();
    let (mut s, _, _) = project(&e);
    for (n, guard) in [allow(&[]), deny(&[MAIN]), allow(&["br1"])]
        .into_iter()
        .enumerate()
    {
        let made = s
            .with_guard(guard, |s| {
                s.create_branch(BranchCreate {
                    tx_id: format!("nb{n}"),
                    author: "test".into(),
                    name: Some(format!("new-{n}")),
                    from: MAIN.into(),
                    at: None,
                    dry_run: false,
                    args_hash: None,
                })
            })
            .unwrap();
        assert_eq!(made.branch.parent.as_deref(), Some(MAIN));
    }
}

/// The guard is about the branch renamed, archived or restored, not the parent whose log
/// takes the record.
#[test]
fn branch_set_is_guarded() {
    let e = env();
    let (mut s, _, other) = project(&e);
    guarded(&mut s, &other, |s| {
        s.set_branch(set("bs1", &other, Some("renamed"), None))
    });
    guarded(&mut s, &other, |s| {
        s.set_branch(set("bs2", &other, None, Some(true)))
    });
    // Restoring an archived branch: still the same id.
    guarded(&mut s, &other, |s| {
        s.set_branch(set("bs3", &other, None, Some(false)))
    });
    let names: Vec<String> = s
        .branches(true)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert!(names.contains(&"renamed".to_string()), "{names:?}");
}

/// A rename between the policy stage (which resolved a name to an id and computed the guard)
/// and the store call cannot slip past it: the store resolves the reference again inside the
/// write transaction, and the guard is about ids.
#[test]
fn rename_between_resolve_and_write_is_caught() {
    let e = env();
    let (mut s, feature, other) = project(&e);
    // The policy stage resolves `feature` to its id and protects it.
    assert_eq!(s.resolve_branch("feature").unwrap(), feature);
    let guard = deny(&[&feature]);
    // Then the branch is renamed, and another branch takes its old name.
    s.set_branch(set("rn1", &feature, Some("renamed"), None))
        .unwrap();
    s.set_branch(set("rn2", &other, Some("feature"), None))
        .unwrap();
    for r in [feature.as_str(), "renamed", "renamed#br1"] {
        let e = refused(&mut s, guard.clone(), &mut |s: &mut Store| {
            let tx = tx_on(s, "w1", r, vec![insert(20, "a1")]);
            s.commit(tx)
        });
        assert_eq!(e.code, "BRANCH_PROTECTED", "{r}: {e:?}");
        assert_eq!(e.details["branch"], feature.as_str());
        assert_eq!(e.details["name"], "renamed");
    }
    // `feature` now names `other`, which the guard does not protect...
    let tx = tx_on(&s, "w2", "feature", vec![insert(20, "a1")]);
    let c = s.with_guard(guard, |s| s.commit(tx)).unwrap();
    assert_eq!(c.tx.branch, other);
    // ...and an allow-list computed for `feature` back when it was br1 refuses it.
    let e = refused(&mut s, allow(&[&feature]), &mut |s: &mut Store| {
        let tx = tx_on(s, "w3", "feature", vec![insert(20, "a1")]);
        s.commit(tx)
    });
    assert_eq!(e.code, "BRANCH_NOT_ALLOWED");
    assert_eq!(e.details["branch"], other.as_str());
}

/// A guard on `br1` applies whether the write names it `feature`, `feature#br1` or `br1`.
#[test]
fn name_and_id_resolve_the_same() {
    let e = env();
    let (mut s, feature, _) = project(&e);
    for r in ["feature", "feature#br1", "br1", "FEATURE"] {
        let e = refused(&mut s, deny(&[&feature]), &mut |s: &mut Store| {
            s.undo(history("u1", r, false))
        });
        assert_eq!(e.code, "BRANCH_PROTECTED", "{r}");
        let e = refused(&mut s, allow(&[MAIN]), &mut |s: &mut Store| {
            s.checkpoint(CheckpointReq {
                tx_id: "cp1".into(),
                author: "test".into(),
                branch: r.into(),
                name: "mark".into(),
                dry_run: false,
                args_hash: None,
            })
        });
        assert_eq!(e.code, "BRANCH_NOT_ALLOWED", "{r}");
    }
    let tx = tx_on(&s, "ok", "feature#br1", vec![insert(20, "a1")]);
    let c = s.with_guard(allow(&[&feature]), |s| s.commit(tx)).unwrap();
    assert_eq!(c.tx.branch, feature);
}

fn recover_all(tx_id: &str) -> RecoverReq {
    RecoverReq {
        tx_id: tx_id.into(),
        author: "rec".into(),
        create: Some(RecoverSel::All),
        import_dir: None,
        dismiss: None,
        dry_run: false,
    }
}

/// Recovery copies a diverged tail onto branches it makes (from the parent, which it only
/// reads, as a create does): no existing branch is a target, so any guard lets it through.
#[test]
fn recover_onto_new_branches_is_not_a_target() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        commit(a, "x1", vec![insert(0, "a1")]);
    });
    let before = a.head(MAIN);
    let r = a
        .with_guard(allow(&[]), |s| s.recover(recover_all("rec")))
        .unwrap();
    assert_eq!(r.created.len(), 1);
    assert!(r.committed.len() >= 2, "{:?}", r.committed);
    assert_eq!(a.head(MAIN), before + 1, "only the `branch` record on main");
}

/// A `branchSet` of a live branch that recovery applies again changes that branch: guarded
/// as `set_branch` is.
#[test]
fn recover_applying_a_branch_set_is_guarded() {
    let e = env();
    let mut s = split(
        &e,
        |a| {
            branch(a, "mk", "side");
        },
        |_| {},
        winner,
    );
    s.a.set_branch(set("xr", "br1", Some("renamed"), None))
        .unwrap();
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    let mut a = s.a;
    let r = guarded(&mut a, "br1", |s| s.recover(recover_all("rec")));
    assert!(r.created.is_empty());
    assert_eq!(r.committed.len(), 1);
    assert_eq!(a.resolve_branch("renamed").unwrap(), "br1");
}

#[test]
fn guard_is_restored_after_the_call() {
    let e = env();
    let (mut s, _, _) = project(&e);
    let inner = s.with_guard(deny(&[MAIN]), |s| {
        let tx = tx_on(s, "g1", MAIN, vec![insert(10, "a1")]);
        s.commit(tx).map_err(|e| e.code().to_string())
    });
    assert_eq!(inner.unwrap_err(), "BRANCH_PROTECTED");
    // Unguarded again once `with_guard` returned, an error included.
    let tx = tx_on(&s, "g1", MAIN, vec![insert(10, "a1")]);
    s.commit(tx).unwrap();
}
