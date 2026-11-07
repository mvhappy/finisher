//! Unit tests of the write path (`Builder::finish`): deferred errors, idempotency keys and
//! `tx.get`.

use std::path::Path;

use fin_core::{CommittedTx, Rational, Tx, new_project};

use super::*;
use crate::backend::{Probed, Snapshot};
use crate::output::exit_code;

/// An empty project at rev 1 (generation `g1`) whose server rejects every commit with
/// `answer` and answers every `tx.get` with `lookup`; it records the generation sent.
struct Rejecting {
    answer: CoreError,
    lookup: TxLookup,
    sent: std::cell::RefCell<Option<Value>>,
}

fn rejecting(answer: CoreError) -> Rejecting {
    Rejecting {
        answer,
        lookup: TxLookup::Unknown,
        sent: Default::default(),
    }
}

impl Backend for Rejecting {
    fn snapshot(&self, _branch: &str) -> Result<Snapshot, CoreError> {
        let rate: Rational = "30".parse()?;
        Ok(Snapshot {
            rev: 1,
            project: new_project("p".into(), "P".into(), rate)?,
            generation: Some(json!({ "incarnation": "g1", "n": 0 })),
        })
    }
    fn commit(&self, _tx: &Tx, generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        *self.sent.borrow_mut() = generation.cloned();
        Err(self.answer.clone())
    }
    fn tx_get(&self, _tx_id: &str) -> Result<TxLookup, CoreError> {
        Ok(self.lookup.clone())
    }
    fn project_call(&self, _tag: &str, _payload: Value) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        unreachable!()
    }
    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        unreachable!()
    }
    fn create_project(
        &self,
        _dir: &Path,
        _name: &str,
        _rate: Rational,
        _idem: Option<&str>,
    ) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn open_project(&self, _dir: &Path, _relocate: bool) -> Result<Value, CoreError> {
        unreachable!()
    }
}

/// What the server committed for `tx_id`: the delete of c9, at rev 2.
fn landed(tx_id: &str) -> CommittedTx {
    serde_json::from_value(json!({
        "rev": 2,
        "tx": { "txId": tx_id, "author": "fin", "branch": "main", "baseRev": 1, "schemaVersion": 2,
                "ops": [{ "op": "item.delete", "item": "c9", "mode": "ripple" }] },
        "bindings": {},
        "diff": { "changes": [], "shifts": [] },
    }))
    .unwrap()
}

/// Like [`Rejecting`], but `tx.get` finds the tx only after the commit was sent.
struct LostAnswer(Rejecting);

impl Backend for LostAnswer {
    fn snapshot(&self, branch: &str) -> Result<Snapshot, CoreError> {
        self.0.snapshot(branch)
    }
    fn commit(&self, tx: &Tx, generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        self.0.commit(tx, generation)
    }
    fn tx_get(&self, tx_id: &str) -> Result<TxLookup, CoreError> {
        Ok(match &*self.0.sent.borrow() {
            Some(_) => TxLookup::Committed(Box::new(landed(tx_id))),
            None => TxLookup::Unknown,
        })
    }
    fn project_call(&self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        self.0.project_call(tag, payload)
    }
    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        self.0.rebuild()
    }
    fn probe(&self, path: &Path) -> Result<Probed, CoreError> {
        self.0.probe(path)
    }
    fn create_project(
        &self,
        d: &Path,
        n: &str,
        r: Rational,
        k: Option<&str>,
    ) -> Result<Value, CoreError> {
        self.0.create_project(d, n, r, k)
    }
    fn open_project(&self, d: &Path, relocate: bool) -> Result<Value, CoreError> {
        self.0.open_project(d, relocate)
    }
}

#[test]
fn a_commit_whose_answer_was_lost_is_found_with_tx_get() {
    // Not known before: the commit is sent, its answer lost; tx.get then finds it.
    let lost = LostAnswer(rejecting(CoreError::new("SERVER_TIMEOUT", "slow")));
    let step = Write::ClipDelete(clip::DeleteArgs {
        clip: "c9".into(),
        mode: None,
    });
    let globals = Globals {
        idem_key: Some("k2".into()),
        ..Globals::default()
    };
    let doc = write(&lost, &globals, step).unwrap();
    assert_eq!(doc["rev"], 2);
    assert!(lost.0.sent.borrow().is_some());
}

#[test]
fn a_retry_with_an_idem_key_is_answered_by_tx_get() {
    let step = || {
        Write::ClipDelete(clip::DeleteArgs {
            clip: "c9".into(),
            mode: None,
        })
    };
    let globals = Globals {
        idem_key: Some("k".into()),
        ..Globals::default()
    };
    // The commit's answer was lost, but tx.get finds it: the command succeeds.
    let mut backend = rejecting(CoreError::new("SERVER_TIMEOUT", "slow"));
    backend.lookup = TxLookup::Committed(Box::new(landed("k")));
    let doc = write(&backend, &globals, step()).unwrap();
    assert_eq!(
        (doc["rev"].clone(), doc["txId"].clone()),
        (json!(2), json!("k"))
    );
    // Found before sending (a retry): nothing is committed again.
    assert!(backend.sent.borrow().is_none());

    // Not known yet: the commit is sent, with the snapshot's generation. Its answer is
    // lost and tx.get still does not know it: the transport error stands.
    let backend = rejecting(CoreError::new("SERVER_UNAVAILABLE", "gone"));
    let e = write(&backend, &globals, step()).unwrap_err();
    assert_eq!(e.code, "SERVER_UNAVAILABLE");
    assert_eq!(
        *backend.sent.borrow(),
        Some(json!({ "incarnation": "g1", "n": 0 }))
    );
}

#[test]
fn an_idem_key_whose_tx_was_set_aside_is_reported_not_resent() {
    let mut backend = rejecting(CoreError::new("CONFLICT", "unused"));
    backend.lookup = TxLookup::SetAside(
        json!({ "txId": "k", "branch": "main", "reason": "SUPERSEDED", "origin": "local" }),
    );
    let globals = Globals {
        idem_key: Some("k".into()),
        ..Globals::default()
    };
    let step = Write::ClipDelete(clip::DeleteArgs {
        clip: "c9".into(),
        mode: None,
    });
    let e = write(&backend, &globals, step).unwrap_err();
    assert_eq!(e.code, "TX_SET_ASIDE");
    assert_eq!(exit_code(&e.code), 4);
    assert_eq!(e.details["reason"], "SUPERSEDED");
    assert!(e.fix.unwrap().contains("new --idem-key"));
    assert!(backend.sent.borrow().is_none(), "never sent again");
}

fn delete_missing_with_key(answer: CoreError) -> CoreError {
    let globals = Globals {
        idem_key: Some("k".into()),
        ..Globals::default()
    };
    let step = Write::ClipDelete(clip::DeleteArgs {
        clip: "c9".into(),
        mode: None,
    });
    write(&rejecting(answer), &globals, step).unwrap_err()
}

#[test]
fn a_deferred_local_error_only_replaces_server_validation_errors() {
    let server_invalid = CoreError::new("NOT_FOUND", "server says no");
    let local = delete_missing_with_key(server_invalid);
    assert_eq!(local.code, "NOT_FOUND");
    assert_ne!(local.message, "server says no");
    assert_eq!(local.details["opIndex"], 0);
    for code in [
        "CONFLICT",
        "BASE_REV_AHEAD",
        "SERVER_SHUTTING_DOWN",
        "ENGINE_CLOSED",
        "INTERNAL",
        "SERVER_TIMEOUT",
    ] {
        let e = delete_missing_with_key(CoreError::new(code, "server"));
        assert_eq!((e.code.as_str(), e.message.as_str()), (code, "server"));
    }
    let no_key = write(
        &rejecting(CoreError::new("CONFLICT", "server")),
        &Globals::default(),
        Write::ClipDelete(clip::DeleteArgs {
            clip: "c9".into(),
            mode: None,
        }),
    );
    assert_eq!(no_key.unwrap_err().code, "NOT_FOUND");
}

#[test]
fn a_rebased_commit_is_the_same_command_but_another_kind_or_clip_is_not() {
    let r = |s: &str| s.parse::<Rational>().unwrap();
    let insert = |at: &str| -> Op {
        fin_core::ItemInsert {
            id: "$clip".into(),
            track: "t1".into(),
            at: r(at),
            duration: r("2"),
            mode: fin_core::InsertMode::Ripple,
            body: fin_core::ItemBody::Clip {
                asset: "a1".into(),
                source_in: r("0"),
                speed: r("1"),
            },
        }
        .into()
    };
    let delete = |clip: &str| -> Op {
        fin_core::ItemDelete {
            item: clip.into(),
            mode: fin_core::DeleteMode::Ripple,
        }
        .into()
    };
    assert!(same_command(&[insert("0")], &[insert("4")]));
    assert!(same_command(&[delete("c1")], &[delete("c1")]));
    assert!(!same_command(&[insert("0")], &[delete("c1")]));
    assert!(!same_command(&[delete("c1")], &[delete("c2")]));
    assert!(!same_command(
        &[delete("c1")],
        &[delete("c1"), delete("c2")]
    ));
}

/// [`landed`] with `argsHash` `hash`.
fn landed_with(tx_id: &str, hash: Option<String>) -> CommittedTx {
    let mut c = landed(tx_id);
    c.tx.args_hash = hash;
    c
}

#[test]
fn idem_key_same_key_different_args_rejected() {
    let globals = Globals {
        idem_key: Some("k".into()),
        ..Globals::default()
    };
    let delete = |clip: &str| {
        Write::ClipDelete(clip::DeleteArgs {
            clip: clip.into(),
            mode: None,
        })
    };
    let ours = args_hash(&[input(&delete("c9"))]);
    // Committed by this same command: answered.
    let mut backend = rejecting(CoreError::new("CONFLICT", "unused"));
    backend.lookup = TxLookup::Committed(Box::new(landed_with("k", Some(ours.clone()))));
    assert_eq!(write(&backend, &globals, delete("c9")).unwrap()["rev"], 2);
    // The key's commit has the same ops but came from another command input (an insert
    // planned `--at end` and one `--at 0` can resolve to the same ops): refused.
    let other = args_hash(&[json!({ "cmd": "clip.delete", "args": { "clip": "c10" } })]);
    backend.lookup = TxLookup::Committed(Box::new(landed_with("k", Some(other))));
    let e = write(&backend, &globals, delete("c9")).unwrap_err();
    assert_eq!(e.code, "IDEM_KEY_REUSED");
    assert_eq!(exit_code(&e.code), 3);
    // A commit without a hash (an older fin): compared by its ops, as before.
    backend.lookup = TxLookup::Committed(Box::new(landed_with("k", None)));
    assert_eq!(write(&backend, &globals, delete("c9")).unwrap()["rev"], 2);
    assert_eq!(
        write(&backend, &globals, delete("c8")).unwrap_err().code,
        "IDEM_KEY_REUSED"
    );
}

#[test]
fn args_hash_is_of_the_command_input() {
    let insert = |json: Value| Write::ClipInsert(serde_json::from_value(json).unwrap());
    let a = input(&insert(
        json!({ "track": "t1", "at": "end", "asset": "a1", "dur": "2s" }),
    ));
    // Key order and absent defaults do not matter; the values do.
    let b = input(&insert(
        json!({ "dur": "2s", "asset": "a1", "at": "end", "track": "t1", "in": null }),
    ));
    assert_eq!(args_hash(std::slice::from_ref(&a)), args_hash(&[b]));
    let c = input(&insert(
        json!({ "track": "t1", "at": "0", "asset": "a1", "dur": "2s" }),
    ));
    assert_ne!(
        args_hash(std::slice::from_ref(&a)),
        args_hash(std::slice::from_ref(&c))
    );
    assert_ne!(args_hash(&[a.clone(), c.clone()]), args_hash(&[c, a]));
    let h = args_hash(&[]);
    assert!(h.starts_with("blake3:") && h.len() == 71, "{h}");
}

#[test]
fn args_hash_uses_the_canonical_media_path() {
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join("a.mov"), b"x").unwrap();
    let add = |p: &Path| {
        input(&Write::AssetAdd(asset::AddArgs {
            path: p.display().to_string(),
        }))
    };
    let direct = add(&media.join("a.mov"));
    // `media/../media/a.mov`, as `asset add ../media/a.mov` typed inside `media/` gives.
    let dotted = add(&media.join("..").join("media").join("a.mov"));
    assert_eq!(
        args_hash(std::slice::from_ref(&direct)),
        args_hash(&[dotted])
    );
    #[cfg(unix)]
    {
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&media, &link).unwrap();
        assert_eq!(
            args_hash(std::slice::from_ref(&direct)),
            args_hash(&[add(&link.join("a.mov"))])
        );
    }
    // A missing file still hashes (by its absolute path).
    let missing = add(&media.join("gone.mov"));
    assert_ne!(args_hash(&[direct]), args_hash(&[missing]));
}

/// Decision 7: the `argsHash` of a 4B `fin clip insert|trim|delete` is unchanged, so a retry
/// across the upgrade is answered from the log. The hashes are the ones 4B's `fin` computed,
/// pinned in the 4B fixture (plan 4C Task 2).
#[test]
fn args_hash_of_4b_inputs_is_unchanged() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fin-store/tests/fixtures/4b/expected.json");
    let expected: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let pinned = expected["argsHashes"].as_array().unwrap();
    assert_eq!(pinned.len(), 3);
    for case in pinned {
        let args = case["args"].clone();
        let step = batch_step(case["cmd"].as_str().unwrap(), args).unwrap();
        assert_eq!(
            input(&step),
            json!({ "cmd": case["cmd"], "args": case["args"] })
        );
        assert_eq!(json!(args_hash(&[input(&step)])), case["hash"], "{case}");
    }
}
