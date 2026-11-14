//! Never blocks (plan D, Decision 18): the data channel drops by entries and by bytes and the
//! writer reports the drops; control messages are never dropped; a slow disk never slows
//! `record`.

mod support;

use std::time::{Duration, Instant};

use fin_core::events::Source;
use fin_journal::{Journal, JournalOptions};
use support::*;

fn commits(all: &[fin_core::events::Envelope]) -> usize {
    of_kind(all, "log.commit").len()
}

#[test]
fn full_channel_drops_and_reports_journal_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        channel: 16,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    assert!(j.pause_writer());
    for i in 0..40 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.resume_writer();
    let all = read_all(&j);
    let written = commits(&all) as u64;
    let dropped = reported_drops(&all);
    assert!((15..=16).contains(&written), "{written}");
    assert_eq!(written + dropped, 40);
    let report = of_kind(&all, "journal.dropped");
    assert_eq!(report.len(), 1);
    assert!(report[0].payload["bytes"].as_u64().unwrap() >= dropped * 100);
    assert_eq!(report[0].payload["since"].as_u64(), Some(T0 + 3_600_000));
    assert_eq!(j.status().dropped, dropped);
}

#[test]
fn byte_bound_drops_large_entries_first_and_counts_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    // What the journal counts: the payload's estimate plus about 200 bytes of envelope.
    let small = commit(0).approx_bytes() + 200;
    let large = commit_with_intent(30).approx_bytes() + 200;
    let o = JournalOptions {
        channel_bytes: 4 * small + large - 100,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    assert!(j.pause_writer());
    for i in 0..4 {
        j.record(Source::FinNode, None, commit(i));
    }
    // Over what is left with four small entries in flight; a fifth small one fits.
    j.record(Source::FinNode, None, commit_with_intent(30));
    j.record(Source::FinNode, None, commit(5));
    j.resume_writer();
    let all = read_all(&j);
    let micros: Vec<_> = of_kind(&all, "log.commit")
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(
        micros,
        [0, 1, 2, 3, 5],
        "the large entry dropped, the small ones not"
    );
    let report = of_kind(&all, "journal.dropped");
    assert_eq!(report[0].payload["count"], 1);
    assert!(report[0].payload["bytes"].as_u64().unwrap() >= large as u64 - 200);
    // In flight returns to zero: everything fits again.
    j.record(Source::FinNode, None, commit_with_intent(30));
    assert_eq!(commits(&read_all(&j)), 6);
}

#[test]
fn control_messages_are_never_dropped_when_data_is_full() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        channel: 8,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    assert!(j.pause_writer());
    for i in 0..20 {
        j.record(Source::FinNode, None, commit(i));
    }
    let sync = {
        let j = j.clone();
        std::thread::spawn(move || {
            j.record_sync(
                Source::Finisherd,
                None,
                session_end(),
                Duration::from_secs(5),
            )
        })
    };
    let flush = {
        let j = j.clone();
        std::thread::spawn(move || j.flush())
    };
    std::thread::sleep(Duration::from_millis(50));
    j.resume_writer();
    assert!(sync.join().unwrap(), "the synchronous record was written");
    flush.join().unwrap().unwrap();
    let all = read_all(&j);
    assert_eq!(of_kind(&all, "session.end").len(), 1);
    assert_eq!(commits(&all) as u64 + reported_drops(&all), 20);
}

#[test]
fn slow_disk_never_blocks_record() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        flush_bytes: 1024,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    j.record(Source::FinNode, None, commit(0));
    j.flush().unwrap();
    j.faults().unwrap().stall_writes(Duration::from_secs(2));
    let mut times = Vec::with_capacity(5000);
    for i in 0..5000 {
        let p = commit(i);
        let t = Instant::now();
        j.record(Source::FinNode, None, p);
        times.push(t.elapsed());
    }
    j.faults().unwrap().stall_writes(Duration::ZERO);
    times.sort();
    let p99 = times[times.len() * 99 / 100];
    let max = *times.last().unwrap();
    // The budget is 5 µs p99 in release (journal_cost); unoptimised test builds get slack.
    let budget = if cfg!(debug_assertions) {
        Duration::from_micros(200)
    } else {
        Duration::from_micros(5)
    };
    assert!(p99 <= budget, "p99 {p99:?}");
    assert!(
        max < Duration::from_millis(100),
        "max {max:?}: record waited on the disk"
    );
    j.flush().unwrap();
}
