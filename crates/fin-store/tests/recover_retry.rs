//! A retried recovery (review #35): `<txId>-<n>` records answer a retry only when they carry
//! the request's identity; other commands' records, or a recovery of other groups, are
//! `IDEM_KEY_REUSED`. Setup as `tests/recover.rs`.

mod integration_support;

use fin_core::{CommittedTx, Op, SCHEMA_VERSION, Tx};
use fin_store::{BranchCreate, MAIN, RecoverReport, RecoverReq, RecoverSel, Store};
use integration_support::diverge::*;
use integration_support::*;

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    })
    .unwrap()
}

fn branch(s: &mut Store, tx_id: &str, name: &str, from: &str) {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
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

/// Review #35: an ordinary branch creation holding `<txId>-1` is not a recovery to answer a
/// retry from, whatever group is asked for; a retry asking for other groups is refused too,
/// and a record under the next id that is not the recovery's is left out of its answer.
#[test]
fn recover_retry_answers_only_from_its_own_records() {
    let e = env();
    let Split { mut a, .. } = diverged(&e, |a| {
        on(a, "x1", MAIN, vec![insert(0, "a1")]);
    });
    branch(&mut a, "recovery-1", "ordinary", MAIN);
    let sel = Some(RecoverSel::Group("does-not-exist".into()));
    let err = fin_core::CoreError::from(a.recover(req("recovery", sel)).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED", "{err:?}");
    assert_eq!(err.details["kind"], "branch", "{err:?}");
    assert!(
        err.fix
            .as_deref()
            .is_some_and(|f| f.contains("another recovery request")),
        "{err:?}"
    );
    let err = fin_core::CoreError::from(
        a.recover(req("recovery", Some(RecoverSel::All)))
            .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED", "{err:?}");
    assert_eq!(
        a.recover_groups().unwrap().len(),
        1,
        "nothing was recovered"
    );

    let group = a.recover_groups().unwrap()[0].id.clone();
    let first = create_all(&mut a, "rec");
    assert_eq!(first.created.len(), 1);
    let sel = Some(RecoverSel::Group(group));
    let err = fin_core::CoreError::from(a.recover(req("rec", sel)).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED", "another selection: {err:?}");
    let mut other_author = req("rec", Some(RecoverSel::All));
    other_author.author = "someone".into();
    let err = fin_core::CoreError::from(a.recover(other_author).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED", "another author: {err:?}");
    let next = format!("rec-{}", first.committed.len() + 1);
    on(&mut a, &next, MAIN, vec![asset("/m/n.mov")]);
    let again = create_all(&mut a, "rec");
    assert_eq!(again.committed, first.committed);
    assert_eq!(again.created, first.created);
    let ours = first.committed[0].tx.args_hash.clone();
    assert!(
        ours.as_deref().is_some_and(|h| h.starts_with("blake3:")),
        "{ours:?}"
    );
    assert!(first.committed.iter().all(|c| c.tx.args_hash == ours));
}
