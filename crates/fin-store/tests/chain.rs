//! Chain resolution (sub-project 4 spec §3.6 steps 4–8, §3.5, §3.9).

use std::collections::BTreeSet;
use std::sync::Arc;

use fin_store::chain::{
    ForkReason, ForkSide, Known, QuarantineReason, Resolution, SegRef, resolve,
};
use fin_store::conflict_names::Provider;
use fin_store::segment::{self, Header};
use fin_store::storage_kind::StorageKind;
use proptest::prelude::*;
use serde_json::json;

const LOCAL: StorageKind = StorageKind::Local;
const SYNC: StorageKind = StorageKind::Sync(Provider::Dropbox);

fn rec(tx: &str, seq: u64, parent: Option<&str>, salt: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "txId": tx, "seq": seq, "parentHash": parent, "v": salt })).unwrap()
}

/// A verified segment built through the real codec.
fn build(
    epoch: u64,
    prev: Option<String>,
    records: Vec<Vec<u8>>,
    replaces: Vec<String>,
    created_at: i64,
) -> SegRef {
    let first: serde_json::Value = serde_json::from_slice(&records[0]).unwrap();
    let first_seq = first["seq"].as_u64().unwrap();
    let header = Header {
        format_version: segment::FORMAT_VERSION,
        min_reader_version: segment::MIN_READER_VERSION,
        project_id: "p1".into(),
        branch: "main".into(),
        writer_id: format!("w{epoch}"),
        epoch,
        first_seq,
        last_seq: first_seq + records.len() as u64 - 1,
        prev_segment_hash: prev,
        op_schema_versions: [1, 1],
        created_at,
        pack: !replaces.is_empty(),
        replaces,
    };
    let refs: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let (bytes, _) = segment::encode(&header, &refs);
    SegRef::new(Arc::new(segment::decode(&bytes).unwrap()))
}

/// Records continuing from `prev`'s last record (or the branch start).
fn records_after(prev: Option<&SegRef>, txs: &[&str], salt: &str) -> Vec<Vec<u8>> {
    let (mut seq, mut parent) = match prev.and_then(|p| p.txs().last()) {
        Some(m) => (m.seq + 1, Some(m.tx_hash.clone())),
        None => (1, None),
    };
    txs.iter()
        .map(|tx| {
            let bytes = rec(tx, seq, parent.as_deref(), salt);
            parent = Some(blake3_of(&bytes));
            seq += 1;
            bytes
        })
        .collect()
}

fn blake3_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// A segment continuing `prev`.
fn next(prev: Option<&SegRef>, epoch: u64, txs: &[&str]) -> SegRef {
    next_salted(prev, epoch, txs, "")
}

fn next_salted(prev: Option<&SegRef>, epoch: u64, txs: &[&str], salt: &str) -> SegRef {
    build(
        epoch,
        prev.map(|p| p.hash().to_string()),
        records_after(prev, txs, salt),
        vec![],
        0,
    )
}

/// A pack of consecutive segments: their records byte for byte, the first one's predecessor.
fn pack(epoch: u64, run: &[&SegRef]) -> SegRef {
    let records = run
        .iter()
        .flat_map(|s| s.records().map(<[u8]>::to_vec))
        .collect();
    let replaces = run.iter().map(|s| s.hash().to_string()).collect();
    build(
        epoch,
        run[0].header().prev_segment_hash.clone(),
        records,
        replaces,
        1,
    )
}

/// A DB that has imported (or exported) exactly `segs`.
fn known(segs: &[&SegRef], kind: StorageKind) -> Known {
    let mut k = Known::empty(kind);
    for s in segs {
        for m in s.txs() {
            k.known_tx.insert(m.tx_id.clone(), m.tx_hash.clone());
            k.head = Some(m.tx_hash.clone());
        }
        k.last_hash = Some(s.hash().to_string());
    }
    k
}

/// Adds `seg`'s records (which need not be in the folder) as the unexported tail.
fn with_tail(mut k: Known, seg: &SegRef, epoch: u64) -> Known {
    for m in seg.txs() {
        k.known_tx.insert(m.tx_id.clone(), m.tx_hash.clone());
        k.tail.push((m.tx_id.clone(), m.tx_hash.clone()));
    }
    k.tail_epoch = epoch;
    k
}

fn applied(r: &Resolution) -> Vec<String> {
    r.apply
        .iter()
        .flat_map(|s| s.txs().iter().map(|m| m.tx_id.clone()))
        .collect()
}

fn quarantined(r: &Resolution) -> Vec<String> {
    r.quarantine
        .iter()
        .flat_map(|(s, _)| s.txs().iter().map(|m| m.tx_id.clone()))
        .collect()
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn no_fork(r: &Resolution) {
    assert_eq!(r.fork, None, "{r:#?}");
}

// ---- plan cases -------------------------------------------------------------------------

#[test]
fn linear_chain_applies_in_order() {
    let s1 = next(None, 1, &["a", "b"]);
    let s2 = next(Some(&s1), 1, &["c"]);
    let s3 = next(Some(&s2), 2, &["d", "e"]);
    let r = resolve(&[s3.clone(), s1.clone(), s2.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b", "c", "d", "e"]));
    assert_eq!(r.apply, vec![s1.clone(), s2.clone(), s3.clone()]);
    assert!(r.apply.iter().all(SegRef::is_whole));
    assert!(r.quarantine.is_empty() && r.waiting.is_empty() && r.mark_exported.is_empty());
    assert!(!r.local_tail_diverged);
    no_fork(&r);

    // A follower that already has s1 imports the rest.
    let r = resolve(&[s1.clone(), s2.clone(), s3.clone()], &known(&[&s1], LOCAL));
    assert_eq!(applied(&r), ids(&["c", "d", "e"]));
    // Up to date: nothing to do.
    let r = resolve(
        &[s1.clone(), s2.clone(), s3.clone()],
        &known(&[&s1, &s2, &s3], LOCAL),
    );
    assert_eq!(r, Resolution::default());
}

#[test]
fn gap_waits() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let s3 = next(Some(&s2), 1, &["c"]);
    let s4 = next(Some(&s3), 1, &["d"]);
    let r = resolve(&[s1.clone(), s3.clone(), s4.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
    assert_eq!(r.waiting, vec![s2.hash().to_string()]);
    assert!(r.quarantine.is_empty());
    no_fork(&r);
    // Once it arrives, everything applies.
    let r = resolve(&[s1, s2, s3, s4], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b", "c", "d"]));
    assert!(r.waiting.is_empty());
}

#[test]
fn fork_equal_epoch() {
    let s1 = next(None, 1, &["a"]);
    let x = next(Some(&s1), 2, &["x"]);
    let y = next(Some(&s1), 2, &["y"]);
    let r = resolve(&[s1.clone(), x.clone(), y.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
    assert!(r.quarantine.is_empty());
    let fork = r.fork.expect("fork");
    assert_eq!(fork.reason, ForkReason::EqualEpoch);
    assert_eq!(fork.after.as_deref(), Some(s1.txs()[0].tx_hash.as_str()));
    assert_eq!(fork.sides.len(), 2);

    // Two first segments at the same epoch fork at the branch start.
    let a = next(None, 1, &["a"]);
    let b = next(None, 1, &["b"]);
    let r = resolve(&[a, b], &Known::empty(LOCAL));
    assert!(r.apply.is_empty());
    let fork = r.fork.expect("fork");
    assert_eq!(fork.after, None);
}

#[test]
fn stale_epoch_quarantined() {
    let s1 = next(None, 1, &["a"]);
    let t = next(Some(&s1), 2, &["t"]);
    let x = next(Some(&s1), 1, &["x"]);
    let x2 = next(Some(&x), 1, &["x2"]);
    let input = [s1.clone(), x.clone(), t.clone(), x2.clone()];
    for k in [
        Known::empty(LOCAL),
        known(&[&s1], LOCAL),
        known(&[&s1, &t], LOCAL),
    ] {
        let r = resolve(&input, &k);
        let mut want = vec![
            (x.clone(), QuarantineReason::StaleEpoch),
            (x2.clone(), QuarantineReason::StaleEpoch),
        ];
        want.sort_by(|a, b| a.0.hash().cmp(b.0.hash()));
        assert_eq!(r.quarantine, want, "{r:#?}");
        no_fork(&r);
    }
    let r = resolve(&input, &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "t"]));
}

#[test]
fn stale_segment_after_takeover_before_first_export_is_quarantined() {
    // B took over at epoch 2 and committed n1, not yet exported. A stale write from epoch 1
    // lands on the same predecessor: it is stale, not a fork, and nothing is applied.
    let s1 = next(None, 1, &["a"]);
    let local = next(Some(&s1), 2, &["n1"]);
    let x = next(Some(&s1), 1, &["x"]);
    let k = with_tail(known(&[&s1], LOCAL), &local, 2);
    let r = resolve(&[s1.clone(), x.clone()], &k);
    assert!(r.apply.is_empty(), "{r:#?}");
    assert_eq!(
        r.quarantine,
        vec![(x.clone(), QuarantineReason::StaleEpoch)]
    );
    assert!(!r.local_tail_diverged);
    assert!(r.mark_exported.is_empty());
    no_fork(&r);

    // Without an unexported tail there is no competing line: the write simply continues
    // the chain (spec §3.6 step 6 compares lines, not single segments).
    let r = resolve(&[s1.clone(), x.clone()], &known(&[&s1], LOCAL));
    assert_eq!(applied(&r), ids(&["x"]));
}

#[test]
fn pack_and_originals_in_grace_window_are_one() {
    let s1 = next(None, 1, &["a", "b"]);
    let s2 = next(Some(&s1), 1, &["c"]);
    let s3 = next(Some(&s2), 2, &["d"]);
    let s4 = next(Some(&s3), 2, &["e"]); // written before compaction: prev = s3
    let p = pack(2, &[&s1, &s2, &s3]);
    let all = ids(&["a", "b", "c", "d", "e"]);

    // Grace window: pack and originals both present.
    let r = resolve(
        &[s1.clone(), s2.clone(), s3.clone(), p.clone(), s4.clone()],
        &Known::empty(LOCAL),
    );
    assert_eq!(applied(&r), all);
    assert_eq!(r.apply, vec![p.clone(), s4.clone()]);
    assert!(r.quarantine.is_empty() && r.waiting.is_empty());
    no_fork(&r);

    // After the grace period: only the pack; s4 resolves through `replaces`.
    let r = resolve(&[p.clone(), s4.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), all);
    no_fork(&r);

    // A follower that stopped inside the packed range resumes inside the pack.
    let r = resolve(&[p.clone(), s4.clone()], &known(&[&s1], LOCAL));
    assert_eq!(applied(&r), ids(&["c", "d", "e"]));
    assert_eq!(r.apply[0].hash(), p.hash());
    assert_eq!(r.apply[0].range(), 2..4);
    // ...also when only some originals are gone.
    let r = resolve(
        &[p.clone(), s3.clone(), s4.clone()],
        &known(&[&s1, &s2], LOCAL),
    );
    assert_eq!(applied(&r), ids(&["d", "e"]));
    assert!(r.waiting.is_empty());
}

#[test]
fn sync_kind_never_auto_resolves() {
    let s1 = next(None, 1, &["a"]);
    let t = next(Some(&s1), 2, &["t"]);
    let x = next(Some(&s1), 1, &["x"]);
    let r = resolve(&[s1.clone(), t.clone(), x.clone()], &Known::empty(SYNC));
    assert_eq!(applied(&r), ids(&["a"]));
    assert!(r.quarantine.is_empty());
    assert_eq!(r.fork.expect("fork").reason, ForkReason::SyncFolder);

    // An unexported tail against a folder line is a fork too, whatever the epochs.
    let local = next(Some(&s1), 3, &["n1"]);
    let k = with_tail(known(&[&s1], SYNC), &local, 3);
    let r = resolve(&[s1.clone(), x.clone()], &k);
    assert!(r.apply.is_empty() && r.quarantine.is_empty() && !r.local_tail_diverged);
    let fork = r.fork.expect("fork");
    assert_eq!(fork.reason, ForkReason::SyncFolder);
    assert_eq!(fork.sides.last(), Some(&ForkSide::LocalTail { epoch: 3 }));

    // Pack plus originals is still one line on sync storage.
    let s2 = next(Some(&s1), 1, &["b"]);
    let p = pack(1, &[&s1, &s2]);
    let r = resolve(&[s1, s2, p], &Known::empty(SYNC));
    assert_eq!(applied(&r), ids(&["a", "b"]));
    no_fork(&r);
}

#[test]
fn crash_after_rename_marks_exported_no_fork() {
    // The export renamed s2 into place, then crashed before marking c and d exported.
    let s1 = next(None, 1, &["a", "b"]);
    let s2 = next(Some(&s1), 2, &["c", "d"]);
    let k = with_tail(known(&[&s1], LOCAL), &s2, 2);
    let r = resolve(&[s1.clone(), s2.clone()], &k);
    assert!(r.apply.is_empty() && r.quarantine.is_empty(), "{r:#?}");
    let h = s2.hash().to_string();
    assert_eq!(
        r.mark_exported,
        vec![("c".into(), h.clone()), ("d".into(), h.clone())]
    );
    assert!(!r.local_tail_diverged);
    no_fork(&r);

    // Part of the tail was exported; the rest still beats a stale write after it.
    let rest = next(Some(&s2), 2, &["e"]);
    let mut k = with_tail(known(&[&s1], LOCAL), &s2, 2);
    k = with_tail(k, &rest, 2);
    let x = next(Some(&s2), 1, &["x"]);
    let r = resolve(&[s1.clone(), s2.clone(), x.clone()], &k);
    assert_eq!(
        r.mark_exported,
        vec![("c".into(), h.clone()), ("d".into(), h)]
    );
    assert_eq!(r.quarantine, vec![(x, QuarantineReason::StaleEpoch)]);
    assert!(r.apply.is_empty());
    no_fork(&r);
}

#[test]
fn same_txid_different_hash_is_fork() {
    let s1 = next(None, 1, &["a", "b"]);
    let s2 = next_salted(Some(&s1), 1, &["c", "a"], "other");
    let r = resolve(&[s1.clone(), s2], &known(&[&s1], LOCAL));
    assert_eq!(applied(&r), ids(&["c"]));
    let fork = r.fork.expect("fork");
    assert_eq!(fork.reason, ForkReason::TxIdReused { tx_id: "a".into() });

    // A txId repeated within the folder chain is the same fork.
    let t = next(None, 1, &["q"]);
    let t2 = next_salted(Some(&t), 1, &["q"], "again");
    let r = resolve(&[t, t2], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["q"]));
    assert_eq!(
        r.fork.unwrap().reason,
        ForkReason::TxIdReused { tx_id: "q".into() }
    );
}

// ---- further cases ----------------------------------------------------------------------

#[test]
fn diverged_tail_loses_to_higher_epoch_line() {
    // Spec §3.6 step 7: this server committed n1 at epoch 1, lost the lease, and the new
    // holder (epoch 2) moved on.
    let s1 = next(None, 1, &["a"]);
    let local = next(Some(&s1), 1, &["n1"]);
    let t = next(Some(&s1), 2, &["t"]);
    let k = with_tail(known(&[&s1], LOCAL), &local, 1);
    let r = resolve(&[s1.clone(), t.clone()], &k);
    assert!(r.local_tail_diverged);
    assert_eq!(applied(&r), ids(&["t"]));
    assert!(r.mark_exported.is_empty());
    no_fork(&r);

    // A client retried n1 on the new holder: same txId, other bytes. The diverged tail is
    // set aside, so importing it is not a txId reuse.
    let t = next_salted(Some(&s1), 2, &["n1"], "retried");
    let r = resolve(&[s1.clone(), t], &k);
    assert!(r.local_tail_diverged);
    assert_eq!(applied(&r), ids(&["n1"]));
    no_fork(&r);

    // A tail committed at epoch 3 (re-acquired, imported, committed) beats epoch 2.
    let t = next(Some(&s1), 2, &["t"]);
    let k = with_tail(known(&[&s1], LOCAL), &local, 3);
    let r = resolve(&[s1, t.clone()], &k);
    assert!(!r.local_tail_diverged && r.apply.is_empty());
    assert_eq!(r.quarantine, vec![(t, QuarantineReason::StaleEpoch)]);
}

#[test]
fn equal_epoch_tail_and_folder_line_fork() {
    let s1 = next(None, 1, &["a"]);
    let local = next(Some(&s1), 2, &["n1"]);
    let y = next(Some(&s1), 2, &["y"]);
    let r = resolve(
        &[s1.clone(), y],
        &with_tail(known(&[&s1], LOCAL), &local, 2),
    );
    assert_eq!(r.fork.expect("fork").reason, ForkReason::EqualEpoch);
    assert!(!r.local_tail_diverged);
}

#[test]
fn multiple_heads_after_a_gap_wait_for_the_gap() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let x = next(Some(&s2), 1, &["x"]);
    let y = next(Some(&s2), 2, &["y"]);
    let r = resolve(&[s1.clone(), x.clone(), y.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
    assert_eq!(r.waiting, vec![s2.hash().to_string()]);
    assert!(r.quarantine.is_empty());
    no_fork(&r);
    let r = resolve(&[s1, s2, x.clone(), y], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b", "y"]));
    assert_eq!(r.quarantine, vec![(x, QuarantineReason::StaleEpoch)]);
}

#[test]
fn unknown_predecessor_with_empty_db_waits() {
    // A fresh importer starts at the branch's first segment (prev null); a snapshot may only
    // seed later, once its txHash is on the resolved chain (spec §3.4).
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let r = resolve(&[s2], &Known::empty(LOCAL));
    assert!(r.apply.is_empty());
    assert_eq!(r.waiting, vec![s1.hash().to_string()]);
}

#[test]
fn head_missing_from_folder_waits() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let s3 = next(Some(&s2), 1, &["c"]);
    let r = resolve(&[s1.clone(), s3], &known(&[&s1, &s2], LOCAL));
    assert!(r.apply.is_empty() && r.fork.is_none());
    assert_eq!(r.waiting, vec![s2.hash().to_string()]);
}

#[test]
fn pack_that_does_not_hold_what_it_replaces_is_malformed() {
    let s1 = next(None, 1, &["a"]);
    let t = next(Some(&s1), 2, &["t"]);
    let x = next(Some(&s1), 1, &["x"]);
    // Claims to replace s1 and the losing x, but holds only s1's records.
    let liar = build(
        3,
        None,
        s1.records().map(<[u8]>::to_vec).collect(),
        vec![s1.hash().to_string(), x.hash().to_string()],
        5,
    );
    let r = resolve(
        &[s1.clone(), t.clone(), x.clone(), liar.clone()],
        &Known::empty(LOCAL),
    );
    assert_eq!(applied(&r), ids(&["a", "t"]));
    assert!(matches!(
        r.quarantine.iter().find(|(s, _)| s == &liar),
        Some((_, QuarantineReason::Malformed(_)))
    ));
    assert!(r.quarantine.contains(&(x, QuarantineReason::StaleEpoch)));
    no_fork(&r);
}

#[test]
fn pack_that_adopted_a_line_carries_its_epoch() {
    // The epoch-3 holder compacted s1 + x: it had resolved x as its line. The pack's epoch is
    // the newest on that line, so the line wins over t (epoch 2) whether or not x's original
    // file is still there.
    let s1 = next(None, 1, &["a"]);
    let t = next(Some(&s1), 2, &["t"]);
    let x = next(Some(&s1), 1, &["x"]);
    let p = pack(3, &[&s1, &x]);
    for input in [
        vec![s1.clone(), t.clone(), x.clone(), p.clone()],
        vec![p.clone(), t.clone()],
    ] {
        let r = resolve(&input, &Known::empty(LOCAL));
        assert_eq!(applied(&r), ids(&["a", "x"]));
        assert_eq!(
            r.quarantine,
            vec![(t.clone(), QuarantineReason::StaleEpoch)]
        );
        no_fork(&r);
    }
    // A follower that had applied t cannot take it back: that is a fork.
    let r = resolve(&[p, t.clone()], &known(&[&s1, &t], LOCAL));
    assert_eq!(r.fork.expect("fork").reason, ForkReason::AppliedLineLost);
}

#[test]
fn applied_line_lost_is_fork() {
    let s1 = next(None, 1, &["a"]);
    let t = next(Some(&s1), 2, &["t"]);
    let x = next(Some(&s1), 3, &["x"]);
    let r = resolve(&[s1.clone(), t.clone(), x], &known(&[&s1, &t], LOCAL));
    assert!(r.apply.is_empty());
    assert_eq!(r.fork.expect("fork").reason, ForkReason::AppliedLineLost);
}

#[test]
fn seq_that_does_not_continue_is_malformed() {
    let s1 = next(None, 1, &["a"]);
    let last = &s1.txs()[0];
    let bad = build(
        1,
        Some(s1.hash().to_string()),
        vec![rec("b", last.seq + 2, Some(&last.tx_hash), "")],
        vec![],
        0,
    );
    let r = resolve(&[s1.clone(), bad.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
    assert!(matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &bad));
}

#[test]
fn prev_that_disagrees_with_parent_hash_is_malformed() {
    let s1 = next(None, 1, &["a", "b"]);
    let first = &s1.txs()[0];
    // Names s1 as predecessor but continues from s1's first record.
    let bad = build(
        2,
        Some(s1.hash().to_string()),
        vec![rec("c", first.seq + 1, Some(&first.tx_hash), "")],
        vec![],
        0,
    );
    let r = resolve(&[s1.clone(), bad.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b"]));
    assert!(matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &bad));
    no_fork(&r);
}

#[test]
fn records_that_do_not_parse_are_malformed() {
    let bad = build(1, None, vec![br#"{"seq":1}"#.to_vec()], vec![], 0);
    assert!(bad.parse_error().is_some());
    let r = resolve(std::slice::from_ref(&bad), &Known::empty(LOCAL));
    assert!(r.apply.is_empty());
    assert!(matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &bad));
}

#[test]
fn epochs_going_back_along_one_line_still_apply() {
    // No competing line, so the epoch rule has nothing to decide (and segments written
    // before a compaction legitimately follow a newer-epoch pack).
    let s1 = next(None, 3, &["a"]);
    let s2 = next(Some(&s1), 2, &["b"]);
    let r = resolve(&[s1, s2], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b"]));
}

#[test]
fn duplicates_and_reexports_are_one_line() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    // The same records in another file (another createdAt, so another hash).
    let s2b = build(
        1,
        Some(s1.hash().to_string()),
        s2.records().map(<[u8]>::to_vec).collect(),
        vec![],
        9,
    );
    assert_ne!(s2.hash(), s2b.hash());
    let s3 = next(Some(&s2b), 1, &["c"]);
    let r = resolve(
        &[s1.clone(), s1.clone(), s2.clone(), s2b, s3, s2],
        &Known::empty(LOCAL),
    );
    assert_eq!(applied(&r), ids(&["a", "b", "c"]));
    assert!(r.quarantine.is_empty() && r.waiting.is_empty());
    no_fork(&r);
}

#[test]
fn long_chain_does_not_recurse() {
    let mut segs = vec![];
    let names: Vec<String> = (0..30_000).map(|i| format!("t{i}")).collect();
    let mut prev: Option<SegRef> = None;
    for chunk in names.chunks(10_000) {
        let txs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let s = next(prev.as_ref(), 1, &txs);
        segs.push(s.clone());
        prev = Some(s);
    }
    let r = resolve(&segs, &Known::empty(LOCAL));
    assert_eq!(applied(&r).len(), 30_000);
    let r = resolve(&segs, &known(&[&segs[0], &segs[1], &segs[2]], LOCAL));
    assert!(r.apply.is_empty());
}

#[test]
fn tail_that_starts_the_branch_competes_at_the_start() {
    let local = next(None, 2, &["n1"]);
    let x = next(None, 1, &["x"]);
    let r = resolve(
        std::slice::from_ref(&x),
        &with_tail(Known::empty(LOCAL), &local, 2),
    );
    assert_eq!(r.quarantine, vec![(x, QuarantineReason::StaleEpoch)]);
    assert!(r.apply.is_empty() && !r.local_tail_diverged);
    let y = next(None, 2, &["y"]);
    let r = resolve(&[y], &with_tail(Known::empty(LOCAL), &local, 1));
    assert!(r.local_tail_diverged);
    assert_eq!(applied(&r), ids(&["y"]));
}

#[test]
fn pack_listing_its_own_successor_is_malformed() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let liar = build(
        2,
        None,
        s1.records().map(<[u8]>::to_vec).collect(),
        vec![s1.hash().to_string(), s2.hash().to_string()],
        3,
    );
    let r = resolve(&[s1, s2, liar.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b"]));
    assert!(matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &liar));
    no_fork(&r);
}

// ---- properties -------------------------------------------------------------------------

/// A randomly built folder: a main line (epochs never decrease), stale lines branching off it
/// with lower epochs, optional pack, duplicate, gap and equal-epoch sibling.
#[derive(Debug, Clone)]
struct Scenario {
    main: Vec<SegRef>,
    stale: Vec<SegRef>,
    extra: Vec<SegRef>,
    main_txs: Vec<String>,
    stale_txs: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct Params {
    lens: Vec<usize>,
    bumps: Vec<bool>,
    stale: Vec<(usize, u64, usize)>,
    tie: Option<usize>,
}

fn params() -> impl Strategy<Value = Params> {
    (
        prop::collection::vec(1usize..4, 1..8),
        prop::collection::vec(any::<bool>(), 8),
        prop::collection::vec((0usize..8, 0u64..5, 1usize..3), 0..4),
        prop::option::of(0usize..8),
    )
        .prop_map(|(lens, bumps, stale, tie)| Params {
            lens,
            bumps,
            stale,
            tie,
        })
}

fn scenario(p: &Params, with_tie: bool) -> Scenario {
    let mut main: Vec<SegRef> = vec![];
    let mut main_txs = vec![];
    let mut epoch = 10;
    for (i, &len) in p.lens.iter().enumerate() {
        if p.bumps[i] {
            epoch += 10;
        }
        let names: Vec<String> = (0..len).map(|j| format!("m{i}_{j}")).collect();
        let txs: Vec<&str> = names.iter().map(String::as_str).collect();
        main.push(next(main.last(), epoch, &txs));
        main_txs.extend(names);
    }
    let mut stale = vec![];
    let mut stale_txs = BTreeSet::new();
    for (k, &(j, drop, len)) in p.stale.iter().enumerate() {
        let j = j % main.len();
        let e = main[j].header().epoch - 1 - drop;
        let mut prev = if j == 0 {
            None
        } else {
            Some(main[j - 1].clone())
        };
        for i in 0..len {
            let name = format!("x{k}_{i}");
            let s = next(prev.as_ref(), e, &[&name]);
            stale_txs.insert(name);
            stale.push(s.clone());
            prev = Some(s);
        }
    }
    let mut extra = vec![];
    if let (true, Some(j)) = (with_tie, p.tie) {
        let j = j % main.len();
        let prev = if j == 0 { None } else { Some(&main[j - 1]) };
        extra.push(next_salted(prev, main[j].header().epoch, &["tie"], "tie"));
    }
    Scenario {
        main,
        stale,
        extra,
        main_txs,
        stale_txs,
    }
}

/// A deterministic shuffle (splitmix64).
fn shuffle<T>(v: &mut [T], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        v.swap(i, (z % (i as u64 + 1)) as usize);
    }
}

fn applied_hashes(r: &Resolution) -> Vec<String> {
    r.apply
        .iter()
        .flat_map(|s| s.txs().iter().map(|m| m.tx_hash.clone()))
        .collect()
}

proptest! {
    #[test]
    fn prop_never_applies_lower_epoch_branch_after_higher(
        p in params(),
        known_n in 0usize..9,
        seed in any::<u64>(),
    ) {
        let s = scenario(&p, false);
        let known_n = known_n % (s.main.len() + 1);
        let refs: Vec<&SegRef> = s.main[..known_n].iter().collect();
        let k = known(&refs, LOCAL);
        let mut input: Vec<SegRef> = s.main.iter().chain(&s.stale).cloned().collect();
        shuffle(&mut input, seed);
        let r = resolve(&input, &k);
        let skip: usize = s.main[..known_n].iter().map(|m| m.txs().len()).sum();
        prop_assert_eq!(applied(&r), s.main_txs[skip..].to_vec());
        prop_assert_eq!(quarantined(&r).into_iter().collect::<BTreeSet<_>>(), s.stale_txs.clone());
        prop_assert!(r.quarantine.iter().all(|(_, q)| *q == QuarantineReason::StaleEpoch));
        prop_assert!(r.fork.is_none() && r.waiting.is_empty() && !r.local_tail_diverged);
    }

    #[test]
    fn prop_resolution_is_order_independent(
        p in params(),
        known_n in 0usize..9,
        gap in prop::option::of(0usize..8),
        dup in prop::option::of(0usize..16),
        sync in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let s = scenario(&p, true);
        let kind = if sync { SYNC } else { LOCAL };
        let known_n = known_n % (s.main.len() + 1);
        let refs: Vec<&SegRef> = s.main[..known_n].iter().collect();
        let k = known(&refs, kind);
        let mut input: Vec<SegRef> = s.main.iter().chain(&s.stale).chain(&s.extra).cloned().collect();
        if let Some(g) = gap {
            let g = g % s.main.len();
            if g >= known_n {
                input.retain(|x| x != &s.main[g]);
            }
        }
        let base = resolve(&input, &k);
        if let (Some(d), false) = (dup, input.is_empty()) {
            let d = d % input.len();
            input.push(input[d].clone());
        }
        shuffle(&mut input, seed);
        prop_assert_eq!(resolve(&input, &k), base.clone());
        input.reverse();
        prop_assert_eq!(resolve(&input, &k), base);
    }

    #[test]
    fn prop_packing_preserves_resolution(
        p in params(),
        known_n in 0usize..9,
        run in (0usize..8, 0usize..8),
        grace in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let s = scenario(&p, false);
        let known_n = known_n % (s.main.len() + 1);
        let refs: Vec<&SegRef> = s.main[..known_n].iter().collect();
        let k = known(&refs, LOCAL);
        let input: Vec<SegRef> = s.main.iter().chain(&s.stale).cloned().collect();
        let before = resolve(&input, &k);

        let (a, b) = (run.0 % s.main.len(), run.1 % s.main.len());
        let (a, b) = (a.min(b), a.max(b));
        let holder = s.main.iter().map(|m| m.header().epoch).max().unwrap();
        let run_refs: Vec<&SegRef> = s.main[a..=b].iter().collect();
        let packed = pack(holder, &run_refs);
        let mut input: Vec<SegRef> = input
            .into_iter()
            .filter(|x| grace || !s.main[a..=b].contains(x))
            .collect();
        input.push(packed);
        shuffle(&mut input, seed);
        let after = resolve(&input, &k);

        prop_assert_eq!(applied_hashes(&after), applied_hashes(&before));
        prop_assert_eq!(
            quarantined(&after).into_iter().collect::<BTreeSet<_>>(),
            quarantined(&before).into_iter().collect::<BTreeSet<_>>()
        );
        prop_assert_eq!(after.fork, before.fork);
        prop_assert!(after.waiting.is_empty());
    }
}
