//! The journal's cost budgets (plan D, Decision 21), measured on this machine:
//! `cargo run -p fin-journal --release --example journal_cost`. Exits 1 on a budget miss.
//!
//! - the disabled gate: ≤ 5 ns a call;
//! - `record` in the caller: p99 ≤ 5 µs for a diagnostics-only `log.commit` and for one with
//!   training on (the intent moved in); the failed-commit path (clone a 100-op request tx,
//!   then record) is measured, not budgeted;
//! - the writer: ≥ 100k diagnostics entries/s and ≥ 20 MiB/s of content entries;
//! - `record_sync` returns within its timeout with the writer stalled;
//! - bytes per entry;
//! - zero entries and zero files when disabled.

use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

use fin_core::events::names::OP_KINDS;
use fin_core::events::payloads::{
    CommitOutcome, CommitRecord, Counts, LogCommit, SessionEnd, TxIntent,
};
use fin_core::events::{
    Actor, ActorKind, AnyPayload, Basis, BranchId, Content, Ctx, IdRef, ProjectId, Purposes, Source,
};
use fin_journal::{Consent, Journal, JournalOptions};
use serde_json::{Value, json};

fn tx(n: usize) -> fin_core::Tx {
    let ops: Vec<Value> = (0..n)
        .map(|i| json!({ "op": "item.trim", "item": format!("c{i}"), "edge": "start", "delta": "1", "mode": "ripple" }))
        .collect();
    serde_json::from_value(json!({
        "txId": "01JC0000000000000000000000", "author": "someone", "branch": "main",
        "baseRev": 41, "schemaVersion": 2, "ops": ops,
    }))
    .unwrap()
}

fn commit(intent: Option<fin_core::Tx>) -> AnyPayload {
    let mut ops = Counts::default();
    ops.add(OP_KINDS.get_or_other("item.trim"), 2);
    LogCommit {
        record: CommitRecord::Edit,
        outcome: CommitOutcome::Rebased,
        code: None,
        ops,
        base_rev: Some(41),
        rebased_over: Some(2),
        conflicts: Counts::default(),
        links: vec![],
        micros: 812,
        intent: intent.map(|t| {
            Content(TxIntent::from_txs([t], &|b: &[u8]| {
                let mut k = [0u8; 16];
                k[..b.len().min(16)].copy_from_slice(&b[..b.len().min(16)]);
                k
            }))
        }),
        intent_omitted: None,
    }
    .into()
}

/// The sample project is open with `collect` default (an unknown project records nothing).
fn allow(j: &Journal) {
    j.set_project_policy(
        "01jb0000000000000000000000",
        Some(fin_core::events::CollectPolicy::Default),
    );
}

fn ctx() -> Ctx {
    Ctx {
        project: ProjectId::parse("01jb0000000000000000000000"),
        branch: BranchId::parse("br1"),
        tx_id: IdRef::classify("01JC0000000000000000000001").ok(),
        author: None,
        actor: Some(Actor {
            kind: ActorKind::Agent,
            session: IdRef::classify("01JC0000000000000000000002").ok(),
            call: None,
        }),
    }
}

fn consent(purposes: Purposes) -> Consent {
    Consent {
        purposes,
        rev: 1,
        basis: Basis::User,
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

struct Report {
    misses: Vec<String>,
}

impl Report {
    fn row(&mut self, what: &str, value: String, budget: Option<(&str, bool)>) {
        match budget {
            Some((b, ok)) => {
                println!(
                    "{what:<52} {value:>14}   budget {b:<12} {}",
                    if ok { "ok" } else { "MISS" }
                );
                if !ok {
                    self.misses.push(what.to_owned());
                }
            }
            None => println!("{what:<52} {value:>14}   (recorded)"),
        }
    }
}

fn pct(v: &mut [Duration], p: usize) -> Duration {
    v.sort();
    v[(v.len() * p / 100).min(v.len() - 1)]
}

/// `record` latency in the caller: 20 rounds of 2,000 calls, a flush between rounds so the
/// channel never fills. The payload is built before the clock starts (it is moved in).
fn record_latency(j: &Journal, make: impl Fn() -> AnyPayload) -> (Duration, Duration) {
    let mut t = Vec::with_capacity(40_000);
    for _ in 0..20 {
        for _ in 0..2_000 {
            let p = make();
            let c = Some(ctx());
            let s = Instant::now();
            j.record(Source::FinNode, c, p);
            t.push(s.elapsed());
        }
        j.flush().unwrap();
    }
    (pct(&mut t, 50), pct(&mut t, 99))
}

fn big() -> JournalOptions {
    // Throughput runs: a channel large enough that nothing drops; the size cap keeps the
    // disk use at 64 MiB.
    JournalOptions {
        channel: 1 << 20,
        channel_bytes: 1 << 31,
        max_total_bytes: 64 << 20,
        ..JournalOptions::default()
    }
}

/// Records `payloads` as fast as the caller can and returns the seconds until the writer
/// has flushed them all.
fn drain_secs(j: &Journal, payloads: Vec<AnyPayload>) -> f64 {
    let s = Instant::now();
    for p in payloads {
        j.record(Source::FinNode, Some(ctx()), p);
    }
    j.flush().unwrap();
    let secs = s.elapsed().as_secs_f64();
    assert_eq!(j.status().dropped, 0, "a throughput run dropped entries");
    secs
}

fn main() {
    let tmp = tempfile::tempdir().unwrap();
    let mut r = Report { misses: Vec::new() };
    println!("fin-journal cost (Decision 21)\n");

    // The disabled gate.
    const N: u32 = 200_000_000;
    let off_open = Journal::open(
        &tmp.path().join("off"),
        Consent::OFF,
        JournalOptions::default(),
    );
    for (name, j) in [
        ("gate(), Journal::disabled()", Journal::disabled()),
        ("gate(), opened with consent off", off_open),
    ] {
        let s = Instant::now();
        let mut on = 0u32;
        for _ in 0..N {
            on += u32::from(black_box(&j).gate().is_some());
        }
        let ns = s.elapsed().as_secs_f64() * 1e9 / f64::from(N);
        assert_eq!(on, 0);
        r.row(name, format!("{ns:.2} ns"), Some(("<= 5 ns", ns <= 5.0)));
        j.close();
    }

    // Zero entries and files when disabled (even when a call site does not check the gate).
    let off_dir = tmp.path().join("off-events");
    let off = Journal::open(&off_dir, Consent::OFF, JournalOptions::default());
    for _ in 0..10_000 {
        off.record(Source::FinNode, Some(ctx()), commit(None));
    }
    off.flush().unwrap();
    let st = off.status();
    off.close();
    let nothing = !off_dir.exists() && st.files == 0 && st.bytes == 0 && st.dropped == 0;
    let seen = format!("{}/{}/{}", st.files, st.bytes, off_dir.exists());
    r.row(
        "disabled: files/bytes/events dir",
        seen,
        Some(("0/0/false", nothing)),
    );

    // `record` in the caller.
    let five = Duration::from_micros(5);
    let diag = tmp.path().join("diag");
    let j = Journal::open(
        &diag,
        consent(Purposes::DIAGNOSTICS),
        JournalOptions::default(),
    );
    allow(&j);
    let (p50, p99) = record_latency(&j, || commit(None));
    r.row(
        "record p50, diagnostics log.commit",
        format!("{p50:?}"),
        None,
    );
    r.row(
        "record p99, diagnostics log.commit",
        format!("{p99:?}"),
        Some(("<= 5 us", p99 <= five)),
    );
    j.close();
    let per = dir_bytes(&diag) as f64 / 40_000.0;
    r.row(
        "bytes per entry, diagnostics log.commit + ctx",
        format!("{per:.0} B"),
        None,
    );

    let train = tmp.path().join("train");
    let j = Journal::open(&train, consent(Purposes::ALL), JournalOptions::default());
    allow(&j);
    let two = tx(2);
    let (p50, p99) = record_latency(&j, || commit(Some(two.clone())));
    r.row(
        "record p50, training log.commit (2-op intent)",
        format!("{p50:?}"),
        None,
    );
    r.row(
        "record p99, training log.commit (2-op intent)",
        format!("{p99:?}"),
        Some(("<= 5 us", p99 <= five)),
    );
    j.flush().unwrap();
    let per = dir_bytes(&train) as f64 / 40_000.0;
    r.row(
        "bytes per entry, training log.commit (2-op)",
        format!("{per:.0} B"),
        None,
    );
    // The failed-commit path: the request tx is cloned before the store call.
    let hundred = tx(100);
    let mut t = Vec::with_capacity(4_000);
    for _ in 0..20 {
        for _ in 0..200 {
            let s = Instant::now();
            let cloned = hundred.clone();
            j.record(Source::FinNode, Some(ctx()), commit(Some(cloned)));
            t.push(s.elapsed());
        }
        j.flush().unwrap();
    }
    r.row(
        "clone 100-op tx + record, p50",
        format!("{:?}", pct(&mut t, 50)),
        None,
    );
    r.row(
        "clone 100-op tx + record, p99",
        format!("{:?}", pct(&mut t, 99)),
        None,
    );
    j.close();

    // Writer throughput.
    const M: usize = 400_000;
    let j = Journal::open(
        &tmp.path().join("tput-diag"),
        consent(Purposes::DIAGNOSTICS),
        big(),
    );
    allow(&j);
    let secs = drain_secs(&j, (0..M).map(|_| commit(None)).collect());
    let rate = M as f64 / secs;
    let shown = format!("{:.0}k/s", rate / 1e3);
    r.row(
        "writer, diagnostics entries/s",
        shown,
        Some((">= 100k/s", rate >= 100_000.0)),
    );
    j.close();

    let probe_dir = tmp.path().join("probe");
    let probe = Journal::open(
        &probe_dir,
        consent(Purposes::ALL),
        JournalOptions::default(),
    );
    allow(&probe);
    let content = tx(150);
    probe.record(Source::FinNode, Some(ctx()), commit(Some(content.clone())));
    probe.close();
    let line = dir_bytes(&probe_dir) as f64;
    const K: usize = 12_000;
    let j = Journal::open(
        &tmp.path().join("tput-content"),
        consent(Purposes::ALL),
        big(),
    );
    allow(&j);
    let secs = drain_secs(&j, (0..K).map(|_| commit(Some(content.clone()))).collect());
    let mib = line * K as f64 / secs / f64::from(1 << 20);
    let what = format!("writer, content entries ({:.1} KiB each)", line / 1024.0);
    r.row(
        &what,
        format!("{mib:.1} MiB/s"),
        Some((">= 20 MiB/s", mib >= 20.0)),
    );
    j.close();

    // record_sync: idle, then with the writer stuck in a 2 s write.
    let opts = JournalOptions::default();
    let j = Journal::open(
        &tmp.path().join("sync"),
        consent(Purposes::DIAGNOSTICS),
        opts,
    );
    allow(&j);
    let end = || -> AnyPayload { SessionEnd { uptime_s: 1 }.into() };
    let mut t = Vec::new();
    for _ in 0..200 {
        let s = Instant::now();
        assert!(j.record_sync(Source::FinNode, None, end(), Duration::from_secs(1)));
        t.push(s.elapsed());
    }
    r.row(
        "record_sync p50, writer idle",
        format!("{:?}", pct(&mut t, 50)),
        None,
    );
    j.faults().unwrap().stall_writes(Duration::from_secs(2));
    j.record(Source::FinNode, None, commit(None));
    let stuck = {
        let j = j.clone();
        std::thread::spawn(move || j.flush())
    };
    std::thread::sleep(Duration::from_millis(100));
    for ms in [250, 1000] {
        let s = Instant::now();
        let ok = j.record_sync(Source::FinNode, None, end(), Duration::from_millis(ms));
        let took = s.elapsed();
        let within = !ok && took < Duration::from_millis(ms + 50);
        let what = format!("record_sync, stalled writer, {ms} ms timeout");
        r.row(&what, format!("{took:?}"), Some(("<= timeout", within)));
    }
    j.faults().unwrap().stall_writes(Duration::ZERO);
    let _ = stuck.join();
    j.close();

    println!();
    if r.misses.is_empty() {
        println!("all budgets met");
    } else {
        println!("budget misses: {}", r.misses.join("; "));
        std::process::exit(1);
    }
}
