//! Shared helpers for the chain resolution tests.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

#[allow(unused_imports)]
pub use fin_store::chain::{
    ForkPoint, ForkReason, ForkSide, Known, QuarantineReason, Resolution, SegRef,
};
use fin_store::conflict_names::Provider;
use fin_store::segment::{self, Header, Replaced};
use fin_store::storage_kind::StorageKind;
use serde_json::json;

pub const LOCAL: StorageKind = StorageKind::Local;
pub const SYNC: StorageKind = StorageKind::Sync(Provider::Dropbox);

pub fn rec(tx: &str, seq: u64, parent: Option<&str>, salt: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "txId": tx, "seq": seq, "parentHash": parent, "v": salt })).unwrap()
}

/// The `replaces` entry for `s`: its hash and its last record.
pub fn rep(s: &SegRef) -> Replaced {
    Replaced {
        hash: s.hash().to_string(),
        last_tx_hash: s.txs().last().unwrap().tx_hash.clone(),
    }
}

/// A verified segment built through the real codec.
pub fn build(
    epoch: u64,
    prev: Option<String>,
    records: Vec<Vec<u8>>,
    replaces: Vec<Replaced>,
    created_at: i64,
) -> SegRef {
    build_on("main", epoch, prev, records, replaces, created_at)
}

/// [`build`] on another branch.
pub fn build_on(
    branch: &str,
    epoch: u64,
    prev: Option<String>,
    records: Vec<Vec<u8>>,
    replaces: Vec<Replaced>,
    created_at: i64,
) -> SegRef {
    let first: serde_json::Value = serde_json::from_slice(&records[0]).unwrap();
    let first_seq = first["seq"].as_u64().unwrap();
    let header = Header {
        format_version: segment::FORMAT_VERSION,
        min_reader_version: segment::MIN_READER_VERSION,
        project_id: "p1".into(),
        branch: branch.into(),
        writer_id: format!("w{epoch}"),
        epoch,
        first_seq,
        last_seq: first_seq + records.len() as u64 - 1,
        prev_segment_hash: prev,
        op_schema_versions: [1, 1],
        created_at,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
        pack: !replaces.is_empty(),
        replaces,
    };
    let refs: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let (bytes, _) = segment::encode(&header, &refs);
    SegRef::new(Arc::new(segment::decode(&bytes).unwrap()))
}

/// Records continuing from `prev`'s last record (or the branch start).
pub fn records_after(prev: Option<&SegRef>, txs: &[&str], salt: &str) -> Vec<Vec<u8>> {
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

pub fn blake3_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// A segment continuing `prev`.
pub fn next(prev: Option<&SegRef>, epoch: u64, txs: &[&str]) -> SegRef {
    next_salted(prev, epoch, txs, "")
}

pub fn next_salted(prev: Option<&SegRef>, epoch: u64, txs: &[&str], salt: &str) -> SegRef {
    build(
        epoch,
        prev.map(|p| p.hash().to_string()),
        records_after(prev, txs, salt),
        vec![],
        0,
    )
}

/// A pack of consecutive segments: their records byte for byte, the first one's predecessor.
pub fn pack(epoch: u64, run: &[&SegRef]) -> SegRef {
    let records = run
        .iter()
        .flat_map(|s| s.records().map(<[u8]>::to_vec))
        .collect();
    let replaces = run.iter().map(|s| rep(s)).collect();
    build(
        epoch,
        run[0].header().prev_segment_hash.clone(),
        records,
        replaces,
        1,
    )
}

/// A DB that has imported (or exported) exactly `segs`.
pub fn known(segs: &[&SegRef], kind: StorageKind) -> Known {
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
pub fn with_tail(mut k: Known, seg: &SegRef, epoch: u64) -> Known {
    for m in seg.txs() {
        k.known_tx.insert(m.tx_id.clone(), m.tx_hash.clone());
        k.tail.push((m.tx_id.clone(), m.tx_hash.clone()));
    }
    k.tail_epoch = epoch;
    k.holds_lease = true;
    k
}

pub fn without_lease(mut k: Known) -> Known {
    k.holds_lease = false;
    k
}

pub fn applied(r: &Resolution) -> Vec<String> {
    r.apply
        .iter()
        .flat_map(|s| s.txs().iter().map(|m| m.tx_id.clone()))
        .collect()
}

pub fn quarantined(r: &Resolution) -> Vec<String> {
    r.quarantine
        .iter()
        .flat_map(|(s, _)| s.txs().iter().map(|m| m.tx_id.clone()))
        .collect()
}

pub fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn no_fork(r: &Resolution) {
    assert_eq!(r.fork, None, "{r:#?}");
}

/// `fin_store::chain::resolve`, plus the accounting invariant.
pub fn resolve(input: &[SegRef], known: &Known) -> Resolution {
    let r = fin_store::chain::resolve(input, known);
    check_accounting(input, known, &r);
    r
}

fn descends(q: &QuarantineReason) -> bool {
    matches!(q, QuarantineReason::Malformed(why) if why.starts_with("descends from"))
}

/// Every input record lands in exactly one of: imported (`apply`), marked exported, set aside
/// (stale, or descending from a malformed segment), pending, or already on the DB line. A
/// record that only lives in a malformed file is covered by that file's `Malformed` entry.
pub fn check_accounting(input: &[SegRef], known: &Known, r: &Resolution) {
    let hashes = |segs: Vec<&SegRef>| -> Vec<String> {
        segs.into_iter()
            .flat_map(|s| s.txs().iter().map(|m| m.tx_hash.clone()))
            .collect()
    };
    let tail: HashMap<&str, &str> = known
        .tail
        .iter()
        .map(|(i, h)| (i.as_str(), h.as_str()))
        .collect();
    let apply = hashes(r.apply.iter().collect());
    let marked: Vec<String> = r
        .mark_exported
        .iter()
        .map(|(id, _)| {
            tail.get(id.as_str())
                .expect("marked records are tail records")
                .to_string()
        })
        .collect();
    let set_aside = hashes(
        r.quarantine
            .iter()
            .filter(|(_, q)| *q == QuarantineReason::StaleEpoch || descends(q))
            .map(|(s, _)| s)
            .collect(),
    );
    let malformed_whole = hashes(
        r.quarantine
            .iter()
            .filter(|(s, q)| {
                matches!(q, QuarantineReason::Malformed(_)) && !descends(q) && s.is_whole()
            })
            .map(|(s, _)| s)
            .collect(),
    );
    let pending = hashes(r.pending.iter().collect());
    let db_line: BTreeSet<&str> = known
        .known_tx
        .iter()
        .filter(|(id, _)| !tail.contains_key(id.as_str()))
        .map(|(_, h)| h.as_str())
        .collect();
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for (name, list) in [
        ("apply", &apply),
        ("marked", &marked),
        ("set aside", &set_aside),
        ("pending", &pending),
    ] {
        for h in list {
            if let Some(other) = seen.insert(h, name) {
                panic!("record {h} is in both {other} and {name}: {r:#?}");
            }
        }
    }
    for h in apply.iter().chain(&marked) {
        assert!(
            !db_line.contains(h.as_str()),
            "DB-line record {h} re-imported or re-marked: {r:#?}"
        );
    }
    for s in input {
        for m in s.txs() {
            let h = m.tx_hash.as_str();
            assert!(
                seen.contains_key(h)
                    || db_line.contains(h)
                    || malformed_whole.iter().any(|x| x == h),
                "record {} ({h}) of {s:?} is in no output: {r:#?}",
                m.tx_id
            );
        }
    }
}
