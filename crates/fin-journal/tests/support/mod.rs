//! Shared helpers for the journal's tests: options with a manual clock, sample payloads and
//! raw views of the files.
#![allow(dead_code)]

pub mod crash_child;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fin_core::events::names::OP_KINDS;
use fin_core::events::payloads::{
    CommitOutcome, CommitRecord, Counts, LogCommit, SessionEnd, TxIntent,
};
use fin_core::events::{AnyPayload, Basis, Content, Envelope, Purposes};
use fin_journal::{Consent, Journal, JournalOptions, ManualClock};
use serde_json::{Value, json};

/// 2026-11-07T00:00:00Z.
pub const T0: u64 = 1_794_009_600_000;
pub const DAY: Duration = Duration::from_secs(86_400);

pub fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(T0 + 3_600_000))
}

/// A stand-in for the install key (D3): a fixed, test-only keyed hash. Never shipped; the
/// journal's default has no key.
pub fn test_key(bytes: &[u8]) -> [u8; 16] {
    let mut out = [0x5au8; 16];
    for (i, b) in bytes.iter().enumerate() {
        out[i % 16] = out[i % 16].rotate_left(3) ^ b;
    }
    out
}

pub fn opts(clock: &Arc<ManualClock>) -> JournalOptions {
    JournalOptions {
        clock: clock.clone(),
        key: Some(Arc::new(test_key)),
        ..JournalOptions::default()
    }
}

pub fn diagnostics() -> Consent {
    Consent {
        purposes: Purposes::DIAGNOSTICS,
        rev: 1,
        basis: Basis::Default,
    }
}

pub fn training() -> Consent {
    Consent {
        purposes: Purposes::ALL,
        rev: 2,
        basis: Basis::User,
    }
}

pub fn events(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("events")
}

/// A diagnostics-only `log.commit`.
pub fn commit(micros: u64) -> AnyPayload {
    let mut ops = Counts::default();
    ops.add(OP_KINDS.get_or_other("item.trim"), 2);
    LogCommit {
        record: CommitRecord::Edit,
        outcome: CommitOutcome::Accepted,
        code: None,
        ops,
        base_rev: Some(41),
        rebased_over: None,
        conflicts: Counts::default(),
        links: vec![],
        micros,
        intent: None,
        intent_omitted: None,
    }
    .into()
}

/// A request tx of `n` ops (`item.trim`), about 90 bytes an op.
pub fn tx(n: usize) -> fin_core::Tx {
    let ops: Vec<Value> = (0..n)
        .map(|i| json!({ "op": "item.trim", "item": format!("c{i}"), "edge": "start", "delta": "1", "mode": "ripple" }))
        .collect();
    serde_json::from_value(json!({
        "txId": "01JC0000000000000000000000", "author": "someone", "branch": "main",
        "baseRev": 41, "schemaVersion": 2, "ops": ops,
    }))
    .unwrap()
}

/// A `log.commit` carrying an intent of `n` ops (content).
pub fn commit_with_intent(n: usize) -> AnyPayload {
    let AnyPayload::LogCommit(mut c) = commit(900) else {
        unreachable!()
    };
    c.intent = Some(Content(TxIntent::from_txs([tx(n)], &test_key)));
    AnyPayload::LogCommit(c)
}

pub fn session_end() -> AnyPayload {
    SessionEnd { uptime_s: 12 }.into()
}

/// The journal files, oldest first.
pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    v.retain(|p| p.file_name().unwrap().to_str().unwrap().starts_with("j-"));
    v.sort();
    v
}

/// Every line of every file as it is on disk, without going through the journal.
pub fn raw_lines(dir: &Path) -> Vec<Envelope> {
    files(dir)
        .iter()
        .flat_map(|p| {
            let s = std::fs::read_to_string(p).unwrap();
            s.lines()
                .map(|l| serde_json::from_str::<Envelope>(l).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn of_kind<'a>(all: &'a [Envelope], kind: &str) -> Vec<&'a Envelope> {
    all.iter().filter(|e| e.kind.as_str() == kind).collect()
}

/// All entries, through the journal (which flushes first).
pub fn read_all(j: &Journal) -> Vec<Envelope> {
    j.read(&fin_journal::Filter::default()).collect()
}

/// The sum of `journal.dropped` counts.
pub fn reported_drops(all: &[Envelope]) -> u64 {
    of_kind(all, "journal.dropped")
        .iter()
        .map(|e| e.payload["count"].as_u64().unwrap())
        .sum()
}
