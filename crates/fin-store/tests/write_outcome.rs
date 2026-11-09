//! The write note (plan D, Decision 22): `Store::write` says whether a write ran, was a retry
//! of a committed txId, or was rolled back as a plan, for each of its eight call sites; the
//! note is read with `take_write_outcome`, which clears it.

mod integration_support;

use fin_core::{SCHEMA_VERSION, Tx};
use fin_store::{
    BranchCreate, BranchSet, CheckpointReq, CherryPickReq, HistoryReq, MAIN, MergeReq, RecoverReq,
    RecoverSel, RevertReq, Store, WriteOutcome,
};
use integration_support::diverge::*;
use integration_support::*;

use WriteOutcome::{Deduped, Planned, Written};

/// Runs `call` three times (a dry run, the write, its retry) and returns the notes.
fn notes<T>(
    s: &mut Store,
    mut call: impl FnMut(&mut Store, bool) -> T,
) -> Vec<Option<WriteOutcome>> {
    let mut out = vec![];
    for dry in [true, false, false] {
        assert_eq!(s.take_write_outcome(), None, "taken before each call");
        let _ = call(s, dry);
        out.push(s.take_write_outcome());
    }
    out
}

const ALL: [Option<WriteOutcome>; 3] = [Some(Planned), Some(Written), Some(Deduped)];

fn tx(id: &str, base_rev: u64, ops: Vec<fin_core::Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

fn branch(s: &mut Store, tx_id: &str, name: &str, dry_run: bool) -> fin_store::BranchCreated {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: MAIN.into(),
        at: None,
        dry_run,
        args_hash: None,
    })
    .unwrap()
}

fn history(tx_id: &str, branch: &str, dry_run: bool) -> HistoryReq {
    HistoryReq {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: branch.into(),
        dry_run,
        args_hash: None,
        to: None,
        after_seq: None,
    }
}

/// A project with an asset, an edit on `main`, a branch `side` with one edit of its own and a
/// branch `third` without it.
fn project(e: &Env) -> (Store, String, String) {
    let mut s = e.create("a");
    commit(&mut s, "t0", vec![asset("/m/0.mov")]);
    commit(&mut s, "t1", vec![insert(0, "a1")]);
    let side = branch(&mut s, "b0", "side", false).branch.id;
    let third = branch(&mut s, "b3", "third", false).branch.id;
    s.commit(Tx {
        branch: side.clone(),
        ..tx("s1", s.head(&side), vec![insert(5, "a1")])
    })
    .unwrap();
    let _ = s.take_write_outcome();
    (s, side, third)
}

#[test]
fn write_outcome_written_deduped_planned() {
    let e = env();
    let (mut s, side, third) = project(&e);
    // A commit has no dry run in the store's public API: the write, then its retry.
    let base = s.head(MAIN);
    let _ = s.commit(tx("c1", base, vec![insert(10, "a1")])).unwrap();
    assert_eq!(s.take_write_outcome(), Some(Written));
    let _ = s.commit(tx("c1", base, vec![insert(10, "a1")])).unwrap();
    assert_eq!(s.take_write_outcome(), Some(Deduped));
    assert_eq!(s.take_write_outcome(), None, "taking clears the note");

    let made = notes(&mut s, |s, dry| branch(s, "b1", "other", dry));
    assert_eq!(made, ALL, "branch create");
    let set = notes(&mut s, |s, dry| {
        s.set_branch(BranchSet {
            tx_id: "bs1".into(),
            author: "test".into(),
            branch: "other".into(),
            name: Some("renamed".into()),
            archived: None,
            dry_run: dry,
            args_hash: None,
        })
        .unwrap()
    });
    assert_eq!(set, ALL, "branch set");
    let undo = notes(&mut s, |s, dry| s.undo(history("u1", MAIN, dry)).unwrap());
    assert_eq!(undo, ALL, "undo");
    let redo = notes(&mut s, |s, dry| s.redo(history("r1", MAIN, dry)).unwrap());
    assert_eq!(redo, ALL, "redo");
    let cp = notes(&mut s, |s, dry| {
        s.checkpoint(CheckpointReq {
            tx_id: "cp1".into(),
            author: "test".into(),
            branch: MAIN.into(),
            name: "mark".into(),
            dry_run: dry,
            args_hash: None,
        })
        .unwrap()
    });
    assert_eq!(cp, ALL, "checkpoint");
    let merged = notes(&mut s, |s, dry| {
        s.merge(MergeReq {
            tx_id: "m1".into(),
            author: "test".into(),
            from: side.clone(),
            into: MAIN.into(),
            skip_conflicting: false,
            dry_run: dry,
            args_hash: None,
        })
        .unwrap()
    });
    assert_eq!(merged, ALL, "merge");
    let reverted = notes(&mut s, |s, dry| {
        s.revert(RevertReq {
            tx_id: "rv1".into(),
            author: "test".into(),
            merge_id: "m1".into(),
            branch: MAIN.into(),
            dry_run: dry,
            args_hash: None,
        })
        .unwrap()
    });
    assert_eq!(reverted, ALL, "revert");
    let picked = notes(&mut s, |s, dry| {
        s.cherry_pick(CherryPickReq {
            tx_id: "p1".into(),
            author: "test".into(),
            origin: "s1".into(),
            into: third.clone(),
            dry_run: dry,
            args_hash: None,
        })
        .unwrap()
    });
    assert_eq!(picked, ALL, "cherry-pick");
}

#[test]
fn recover_create_notes_and_listing_writes_nothing() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        commit(a, "x1", vec![insert(0, "a1")]);
    });
    let req = |tx_id: &str, create: Option<RecoverSel>, dry_run| RecoverReq {
        tx_id: tx_id.into(),
        author: "rec".into(),
        create,
        import_dir: None,
        dismiss: None,
        dry_run,
    };
    let _ = a.take_write_outcome();
    a.recover(req("", None, false)).unwrap();
    assert_eq!(a.take_write_outcome(), None, "a listing reaches no write");
    let created = notes(&mut a, |s, dry| {
        s.recover(req("rec", Some(RecoverSel::All), dry)).unwrap()
    });
    assert_eq!(created, ALL, "recover create");
}

#[test]
fn normalise_failure_retry_is_deduped() {
    let e = env();
    let mut s = e.create("a");
    commit(&mut s, "t0", vec![asset("/m/0.mov")]);
    commit(&mut s, "t1", vec![insert(0, "a1")]);
    let _ = s.take_write_outcome();
    // A retry in a shape normalising refuses (an item op in a schema-1 tx) is still answered
    // from the log.
    let retry = Tx {
        schema_version: 1,
        ..tx("t1", 1, vec![insert(0, "a1")])
    };
    assert!(fin_core::normalize(retry.clone()).is_err());
    let answered = s.commit(retry).unwrap();
    assert_eq!(answered.tx.tx_id, "t1");
    assert_eq!(s.take_write_outcome(), Some(Deduped));
    // A new txId in that shape is refused and reaches no write.
    let err = s.commit(Tx {
        schema_version: 1,
        ..tx("t9", 2, vec![insert(0, "a1")])
    });
    assert!(err.is_err());
    assert_eq!(s.take_write_outcome(), None);
}

#[test]
fn a_failed_write_leaves_no_note() {
    let e = env();
    let mut s = e.create("a");
    commit(&mut s, "t0", vec![asset("/m/0.mov")]);
    let _ = s.take_write_outcome();
    // Base ahead of the head: refused inside the write.
    assert!(s.commit(tx("t1", 5, vec![insert(0, "a1")])).is_err());
    assert_eq!(s.take_write_outcome(), None);
}
