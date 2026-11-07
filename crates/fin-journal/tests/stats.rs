//! Per-minute histograms (plan D, Decision 19): one `rpc.stats` or `eval.stats` entry per key
//! per window, and at most `MAX_KEYS` keys per metric with the rest folded into `other`.

mod support;

use std::collections::BTreeSet;
use std::time::Duration;

use fin_core::events::names::{EVAL_KEYS, RPC_METHODS, TABLES};
use fin_core::events::{Code, Name};
use fin_journal::{Journal, MAX_KEYS, Metric};
use serde_json::json;
use support::*;

#[test]
fn stats_window_writes_one_entry_per_key_per_minute() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    c.advance(Duration::from_millis(12_345)); // mid-minute
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let open = RPC_METHODS.get_or_other("project.open");
    let status = RPC_METHODS.get_or_other("project.status");
    let frame = EVAL_KEYS.get_or_other("eval.frame");
    for us in [0, 3, 1000] {
        j.observe(Metric::Rpc, open, us, None);
    }
    j.observe(Metric::Rpc, open, 5, Code::parse("CONFLICT"));
    j.observe(Metric::Rpc, status, 70, None);
    j.observe(Metric::Eval, frame, 16_000, None);
    j.flush().unwrap();
    assert!(read_all(&j).is_empty(), "nothing before the window closes");
    c.advance(Duration::from_secs(60));
    j.flush().unwrap();
    // A sample in the next window.
    j.observe(Metric::Rpc, open, 2, None);
    j.flush().unwrap();
    let all = read_all(&j);
    let rpc = of_kind(&all, "rpc.stats");
    let eval = of_kind(&all, "eval.stats");
    assert_eq!((rpc.len(), eval.len()), (2, 1));
    let start = T0 + 3_600_000; // the minute of the first sample
    let o = &rpc
        .iter()
        .find(|e| e.payload["key"] == "project.open")
        .unwrap()
        .payload;
    assert_eq!(o["histogram"]["start"], start);
    assert_eq!(o["histogram"]["windowMs"], 60_000);
    assert_eq!(o["histogram"]["count"], 4);
    assert_eq!(o["histogram"]["maxMicros"], 1000);
    assert_eq!(o["histogram"]["errors"], json!({ "CONFLICT": 1 }));
    // 0 and 1 µs share bucket 0; 3 → 1, 5 → 2, 1000 → 9.
    assert_eq!(
        o["histogram"]["buckets"],
        json!([1, 1, 1, 0, 0, 0, 0, 0, 0, 1])
    );
    assert_eq!(eval[0].payload["key"], "eval.frame");
    assert_eq!(rpc[0].source, fin_core::events::Source::Finisherd);
    assert_eq!(eval[0].source, fin_core::events::Source::FinNode);

    // Close writes the open window, shortened to its real length.
    c.advance(Duration::from_secs(10));
    j.close();
    let j = Journal::open(&events(&dir), fin_journal::Consent::OFF, opts(&c));
    let all = read_all(&j);
    let rpc = of_kind(&all, "rpc.stats");
    assert_eq!(rpc.len(), 3);
    assert_eq!(rpc[2].payload["histogram"]["start"], start + 60_000);
    assert_eq!(rpc[2].payload["histogram"]["count"], 1);
    assert_eq!(rpc[2].payload["histogram"]["windowMs"], 22_345);
}

#[test]
fn stats_overflow_folds_into_other() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let keys: BTreeSet<Name> = TABLES
        .iter()
        .flat_map(|t| t.entries().iter().map(|e| t.get_or_other(e)))
        .filter(|n| *n != Name::OTHER)
        .collect();
    assert!(keys.len() > MAX_KEYS + 10, "{} distinct names", keys.len());
    let extra = keys.len() - MAX_KEYS;
    for k in &keys {
        j.observe(Metric::Rpc, *k, 1, None);
    }
    j.observe(Metric::Eval, *keys.iter().next().unwrap(), 1, None);
    c.advance(Duration::from_secs(60));
    j.flush().unwrap();
    let all = read_all(&j);
    let rpc = of_kind(&all, "rpc.stats");
    assert_eq!(rpc.len(), MAX_KEYS + 1, "256 keys and other");
    let other = rpc.iter().find(|e| e.payload["key"] == "other").unwrap();
    assert_eq!(other.payload["histogram"]["count"], extra as u64);
    assert_eq!(
        of_kind(&all, "eval.stats").len(),
        1,
        "a separate budget per metric"
    );
    let total: u64 = rpc
        .iter()
        .map(|e| e.payload["histogram"]["count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, keys.len() as u64);
}

#[test]
fn stats_are_not_kept_while_off() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), fin_journal::Consent::OFF, opts(&c));
    j.observe(Metric::Rpc, Name::OTHER, 1, None);
    c.advance(Duration::from_secs(120));
    j.flush().unwrap();
    j.close();
    assert!(!events(&dir).exists());
}
