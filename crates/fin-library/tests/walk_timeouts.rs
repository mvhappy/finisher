//! The walker's spin-up-tolerant deadlines (plan 5D Task 4, Decision 18), scaled: the first
//! stat's 60 s and the per-entry 30 s become milliseconds; the fake source really sleeps its
//! delays, the walker never sleeps through a deadline.

mod walk_util;

use std::sync::Arc;
use std::time::{Duration, Instant};

use fin_library::source::fake::FakeSource;
use fin_library::{RootState, Source};
use serde_json::json;
use walk_util::*;

fn fake(spec: &serde_json::Value) -> Arc<dyn Source> {
    Arc::new(FakeSource::from_json(&spec.to_string()).unwrap())
}

#[test]
fn first_stat_45s_of_60_is_online() {
    let mut spec = tree();
    spec["statDelayMs"] = json!(450);
    let mut o = opts(4);
    o.timing = timing(600, 5_000);
    let (r, batches) = run(fake(&spec), full(1, None), &o);
    assert_eq!(r.unwrap().listed, 7);
    assert_eq!(done_paths(&batches).len(), 7);
}

#[test]
fn first_stat_past_60s_is_offline() {
    let mut spec = tree();
    spec["statDelayMs"] = json!(2_000);
    let mut o = opts(4);
    o.timing = timing(600, 5_000);
    let t = Instant::now();
    let (r, batches) = run(fake(&spec), full(1, None), &o);
    assert!(matches!(offline(&r), Some(RootState::Timeout)), "{r:?}");
    assert!(batches.is_empty());
    assert!(
        t.elapsed() < Duration::from_millis(1_500),
        "waited {:?}",
        t.elapsed()
    );
}

#[test]
fn entry_timeout_marks_dir_unreadable_not_missing() {
    let mut spec = tree();
    // `b` hangs far past the per-entry deadline; its siblings and cousins still list.
    spec["children"][1]["readDelayMs"] = json!(3_000);
    let mut o = opts(4);
    o.timing = timing(5_000, 300);
    let t = Instant::now();
    let (r, batches) = run(fake(&spec), full(1, None), &o);
    let s = r.unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(2_000),
        "waited {:?}",
        t.elapsed()
    );
    assert_eq!(
        done_paths(&batches),
        ["", "a", "b!TIMEOUT", "c", "a/x", "a/y"]
    );
    assert_eq!((s.timeouts, s.stuck, s.incomplete), (1, 1, 1));
    let b = dones(&batches)
        .into_iter()
        .find(|d| d.dir == rel("b"))
        .unwrap();
    assert!(!b.complete);
    assert_eq!(b.entries, 0);
    assert!(entries(&batches).iter().all(|e| e.dir != rel("b")));
}

#[test]
fn every_thread_stuck_is_offline() {
    let mut spec = tree();
    for i in 0..3 {
        spec["children"][i]["readDelayMs"] = json!(3_000);
    }
    let mut o = opts(2);
    o.timing = timing(5_000, 200);
    let t = Instant::now();
    let (r, _) = run(fake(&spec), full(1, None), &o);
    assert!(matches!(offline(&r), Some(RootState::Timeout)), "{r:?}");
    assert!(
        t.elapsed() < Duration::from_millis(2_000),
        "waited {:?}",
        t.elapsed()
    );
}

#[test]
fn root_listing_timeout_is_offline() {
    let mut spec = tree();
    spec["readDelayMs"] = json!(3_000);
    let mut o = opts(4);
    o.timing = timing(5_000, 200);
    let (r, batches) = run(fake(&spec), full(1, None), &o);
    assert!(matches!(offline(&r), Some(RootState::Timeout)), "{r:?}");
    assert!(batches.is_empty());
}

#[test]
fn slow_flush_reports_progress_by_time() {
    // Small listings that each take a while: batches go out by time, not by size.
    let slow: Vec<serde_json::Value> = (0..6)
        .map(|i| {
            let mut d = dir(&format!("d{i}"), i, vec![file("f", 1)]);
            d["readDelayMs"] = json!(60);
            d
        })
        .collect();
    let mut o = opts(1);
    o.timing = timing(5_000, 5_000);
    o.timing.flush = Duration::from_millis(100);
    let (r, batches) = run(fake(&root(slow)), full(1, None), &o);
    r.unwrap();
    assert!(batches.len() >= 3, "{} batches", batches.len());
}

#[test]
fn cancel_during_first_stat() {
    let mut spec = tree();
    spec["statDelayMs"] = json!(3_000);
    let cancel = fin_library::CancelToken::new();
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        c.cancel();
    });
    let t = Instant::now();
    let r = fin_library::walk(
        fake(&spec),
        full(1, None),
        &opts(2),
        &mut |_| Ok(()),
        &cancel,
    );
    assert!(matches!(r, Err(fin_library::WalkError::Cancelled)), "{r:?}");
    assert!(
        t.elapsed() < Duration::from_millis(1_500),
        "waited {:?}",
        t.elapsed()
    );
}

#[test]
fn slow_sink_never_times_out_finished_dirs() {
    // Eight quick directories on four threads; one sink call takes twice the per-entry
    // deadline while the others finish. Finished reads are not stuck reads.
    let dirs: Vec<serde_json::Value> = (0..8)
        .map(|i| dir(&format!("d{i}"), i, vec![file("f", 1)]))
        .collect();
    let mut o = opts(4);
    o.timing = timing(5_000, 300);
    o.batch_entries = 1;
    let mut slept = false;
    let mut batches = Vec::new();
    let r = fin_library::walk(
        fake(&root(dirs)),
        full(1, None),
        &o,
        &mut |b| {
            let below = b.entries.iter().any(|e| !e.dir.is_root());
            if below && !slept {
                slept = true;
                std::thread::sleep(Duration::from_millis(600));
            }
            batches.push(b);
            Ok(())
        },
        &fin_library::CancelToken::new(),
    );
    let s = r.unwrap();
    assert!(slept);
    assert_eq!((s.timeouts, s.incomplete, s.listed), (0, 0, 9), "{s:?}");
    assert!(done_paths(&batches).iter().all(|p| !p.contains('!')));
}
