//! The commit path's cost with the journal on (plan D, Decision 21), measured on this machine:
//! `cargo run -p fin-node --release --example commit_overhead --features test-hooks`. Exits 1
//! on a budget miss.
//!
//! It runs `commit_log::commit`, the function `Engine.commit` runs inside `with_store` (this
//! file includes `src/commit_log.rs` and `src/journal/ctx.rs`, which hold no napi), on a
//! 1k-item project, with the journal off (twice: `off` and `off-2`), with diagnostics on, with
//! training on, and with diagnostics on and the writer stalled (an injected slow disk,
//! `test-hooks`). Each round makes one commit per mode, each round starting one mode later, so
//! drift (the log grows) and position effects fall on all of them alike.
//!
//! **The gate** is the median over the rounds of each mode's commit minus the same round's
//! `off` commit (paired differences), against off's p50: diagnostics ≤ 2 % and ≤ 20 µs (the
//! stalled writer too), training ≤ 5 % and ≤ 50 µs. Two p50s of ~2 ms commits differ by
//! ±25 µs from run to run with nothing changed; the paired median does not. `off-2` minus
//! `off` is printed on every run: the noise floor the gate works within. The logging work
//! alone is timed too ([`direct`]): what `commit_log::commit` adds around the store call.

#![allow(dead_code)]

#[path = "../src/journal/ctx.rs"]
mod ctx;
mod journal {
    pub(crate) use super::ctx;
}
#[path = "../src/commit_log.rs"]
mod commit_log;

use std::path::{Path, PathBuf};
use std::time::Instant;

use fin_core::events::{Basis, CollectPolicy, Purposes};
use fin_core::{Op, Rational, Tx};
use fin_journal::{Consent, Filter, Journal, JournalOptions};
use fin_store::Store;

const ITEMS: usize = 1_000;
const ROUNDS: usize = 3_000;
/// Iterations of the logging work alone ([`direct`]).
const DIRECT: usize = 2_000;
const META: &str =
    r#"{"actor":{"kind":"agent","session":"bench-session","call":"01JC0000000000000000000009"}}"#;

fn tx(id: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "bench".into(),
        branch: "main".into(),
        base_rev,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

/// A project with one asset and `ITEMS` clips on one track.
fn project(data: &Path, dir: &Path) -> Store {
    let mut s = Store::create(data, dir, "Bench", Rational::from_int(30), None).expect("create");
    let video = fin_core::VideoInfo {
        width: 1920,
        height: 1080,
        rate: Rational::from_int(30),
    };
    let asset: Op = fin_core::AssetAdd::media(
        "$a",
        "/media/a.mov",
        Rational::from_int(100_000),
        Some(video),
        None,
        "fp",
    )
    .into();
    s.commit(tx("seed-asset", 0, vec![asset])).expect("asset");
    for chunk in 0..ITEMS / 100 {
        let ops = (0..100)
            .map(|i| {
                let n = (chunk * 100 + i) as i64;
                Op::from(fin_core::ItemInsert {
                    id: format!("$c{n}"),
                    track: "t1".into(),
                    at: Rational::from_int(n * 2),
                    duration: Rational::from_int(1),
                    mode: fin_core::InsertMode::Ripple,
                    body: fin_core::ItemBody::Clip {
                        asset: "a1".into(),
                        source_in: Rational::from_int(0),
                        speed: Rational::from_int(1),
                    },
                })
            })
            .collect();
        let head = s.head("main");
        s.commit(tx(&format!("seed-{chunk}"), head, ops))
            .expect("items");
    }
    s
}

fn consent(purposes: Purposes) -> Consent {
    Consent {
        purposes,
        rev: 1,
        basis: Basis::User,
    }
}

struct Mode {
    name: &'static str,
    journal: Journal,
    /// One commit's µs per round (rounds after the first), in round order.
    samples: Vec<f64>,
}

impl Mode {
    fn new(name: &'static str, journal: Journal) -> Mode {
        Mode {
            name,
            journal,
            samples: vec![],
        }
    }
}

fn p(sorted: &[f64], q: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// The median of `m`'s commit minus the same round's `off` commit.
fn paired(m: &Mode, off: &Mode) -> f64 {
    let d: Vec<f64> = m
        .samples
        .iter()
        .zip(&off.samples)
        .map(|(a, b)| a - b)
        .collect();
    p(&sorted(&d), 0.5)
}

/// The logging work alone, `DIRECT` times: the attempt before the call and the
/// classification and record after it, on clones of `c` made beforehand. Returns p50 / p99 µs.
fn direct(j: &Journal, project: &str, request: &Tx, c: &fin_core::CommittedTx) -> (f64, f64) {
    let clones: Vec<_> = (0..DIRECT).map(|_| c.clone()).collect();
    let mut us = Vec::with_capacity(DIRECT);
    for c in clones {
        let t = Instant::now();
        if let Some(a) = commit_log::Attempt::edit(j, project, request, Some(META)) {
            a.committed(Some(fin_store::WriteOutcome::Written), 0, c);
        }
        us.push(t.elapsed().as_secs_f64() * 1e6);
    }
    us.sort_by(f64::total_cmp);
    (p(&us, 0.5), p(&us, 0.99))
}

fn main() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("fin-commit-overhead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let mut s = project(&root.join("data"), &root.join("project"));
    let id = s.project_id().to_string();
    let open = |name: &str, purposes| {
        Journal::open(
            &root.join(name),
            consent(purposes),
            JournalOptions::default(),
        )
    };
    let diag = open("events-diagnostics", Purposes::DIAGNOSTICS);
    let training = open("events-training", Purposes::ALL);
    let stalled = open("events-stalled", Purposes::DIAGNOSTICS);
    let scopes: Vec<_> = [&diag, &training, &stalled]
        .iter()
        .map(|j| j.open_project(&id, CollectPolicy::Default))
        .collect();
    assert!(stalled.pause_writer(), "the writer parks");
    let mut modes = vec![
        Mode::new("off", Journal::disabled()),
        Mode::new("off-2", Journal::disabled()),
        Mode::new("diagnostics", diag.clone()),
        Mode::new("training", training.clone()),
        Mode::new("stalled", stalled.clone()),
    ];
    let count = modes.len();
    let item = s.snapshot("main").expect("snapshot").1.timelines[0].tracks[0].items[ITEMS / 2]
        .id
        .clone();
    let mut n = 0u64;
    let mut sign = 1;
    for round in 0..ROUNDS {
        for k in 0..modes.len() {
            let m = &mut modes[(round + k) % count];
            {
                let delta = if sign > 0 { "1/30" } else { "-1/30" };
                sign = -sign;
                let op = Op::from(fin_core::ItemTrim {
                    item: item.clone(),
                    edge: fin_core::Edge::End,
                    delta: delta.parse().expect("a time"),
                    mode: fin_core::TrimMode::Ripple,
                    alone: false,
                });
                n += 1;
                let t = tx(&format!("01JC{n:022}"), s.head("main"), vec![op]);
                let started = Instant::now();
                commit_log::commit(&mut s, &m.journal, t, None, Some(META), |_, _| {})
                    .expect("commit");
                let us = started.elapsed().as_secs_f64() * 1e6;
                if round > 0 {
                    m.samples.push(us);
                }
            }
        }
    }
    let request = tx("01JC9999999999999999999999", s.head("main"), vec![]);
    let last = s
        .ops_since("main", s.head("main") - 1)
        .expect("the last commit");
    let direct_costs: Vec<(&str, (f64, f64))> = [
        ("diagnostics", &diag),
        ("training", &training),
        ("stalled", &stalled),
    ]
    .into_iter()
    .map(|(name, j)| {
        (
            name,
            direct(
                j,
                &id,
                &Tx {
                    ops: last[0].tx.ops.clone(),
                    ..request.clone()
                },
                &last[0],
            ),
        )
    })
    .collect();
    stalled.resume_writer();
    let mut ok = true;
    let off = &modes[0];
    let off_p50 = p(&sorted(&off.samples), 0.5);
    println!(
        "commit on a {ITEMS}-item project, {} rounds (µs; overhead = median paired difference \
         against off, p50 difference for reference):",
        off.samples.len()
    );
    for m in &modes {
        let all = sorted(&m.samples);
        let p50 = p(&all, 0.5);
        let over = paired(m, off);
        let pct = over / off_p50 * 100.0;
        let budget = match m.name {
            "off" | "off-2" => None,
            "training" => Some((5.0, 50.0)),
            _ => Some((2.0, 20.0)),
        };
        let verdict = match budget {
            None if m.name == "off" => String::new(),
            None => "   (noise floor)".to_string(),
            Some((max_pct, max_us)) => {
                let pass = pct <= max_pct && over <= max_us;
                ok &= pass;
                format!(
                    "   budget ≤ {max_pct} % and ≤ {max_us} µs: {}",
                    if pass { "ok" } else { "MISS" }
                )
            }
        };
        println!(
            "  {:<12} p50 {p50:>7.1} p90 {:>7.1}   overhead {over:>+6.1} µs ({pct:>+5.2} %), \
             p50 diff {:>+6.1}{verdict}",
            m.name,
            p(&all, 0.9),
            p50 - off_p50,
        );
    }
    println!("the logging work alone, {DIRECT} times (p50 / p99 µs):");
    for (name, (p50, p99)) in &direct_costs {
        println!("  {name:<12} {p50:>8.2} / {p99:>8.2}");
    }
    for (name, j) in [
        ("diagnostics", &diag),
        ("training", &training),
        ("stalled", &stalled),
    ] {
        let _ = j.flush();
        let written = j
            .read(&Filter {
                kinds: vec!["log.commit".into()],
                ..Filter::default()
            })
            .count();
        println!("  {name}: {written} log.commit entries on disk");
    }
    drop(scopes);
    for j in [diag, training, stalled] {
        j.close();
    }
    let _ = std::fs::remove_dir_all(&root);
    if !ok {
        eprintln!("a commit-overhead budget was missed");
        std::process::exit(1);
    }
}
