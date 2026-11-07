//! The writer's review fixes (plan D, Task D2 review): a suspended journal's retries make no
//! new files, a withdrawal holds for lines already buffered, drops are reported at most once
//! per flush interval, a due retry recovers from `maintain`, and the sizes stay consistent
//! after a suspension discards the buffer.

mod support;

use std::time::Duration;

use fin_core::events::Source;
use fin_journal::{JournalOptions, JournalState};
use support::*;

#[test]
fn suspended_retries_make_at_most_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = fin_journal::Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.faults().unwrap().fail_writes(true);
    for i in 0..20 {
        j.record(Source::FinNode, None, commit(i));
        assert!(j.flush().is_err(), "retry {i}");
        c.advance(Duration::from_secs(61));
    }
    let made = files(&events(&dir));
    assert!(made.len() <= 1, "{made:?}");
    assert_eq!(j.status_now().bytes, 0);
    // The disk comes back: the reused file takes the next entry.
    j.faults().unwrap().fail_writes(false);
    j.record(Source::FinNode, None, commit(99));
    j.flush().unwrap();
    assert_eq!(files(&events(&dir)).len(), 1);
    assert_eq!(of_kind(&raw_lines(&events(&dir)), "log.commit").len(), 1);
}

#[test]
fn withdrawal_holds_for_buffered_lines() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        flush_every: Duration::from_secs(3600),
        flush_bytes: 1 << 20,
        ..opts(&c)
    };
    let j = fin_journal::Journal::open(&events(&dir), training(), o);
    for _ in 0..5 {
        j.record(Source::FinNode, None, commit_with_intent(3));
    }
    j.record(Source::FinNode, None, commit(7));
    // The writer buffers the six lines (with their content) and parks before any flush.
    assert!(j.pause_writer());
    j.set_gate_for_test(diagnostics());
    j.resume_writer();
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    let commits = of_kind(&all, "log.commit");
    assert_eq!(commits.len(), 6, "the metadata stays");
    for e in &commits {
        assert!(e.payload.get("intent").is_none(), "content on disk: {e:?}");
        assert_eq!(
            e.consent.purposes,
            fin_core::events::Purposes::DIAGNOSTICS,
            "the stamp claims no more than the gate"
        );
    }
    // Nothing left in force: buffered lines are dropped, never written.
    j.record(Source::FinNode, None, commit(8));
    assert!(j.pause_writer());
    j.set_gate_for_test(fin_journal::Consent::OFF);
    j.resume_writer();
    j.flush().unwrap();
    assert_eq!(of_kind(&raw_lines(&events(&dir)), "log.commit").len(), 6);
}

#[test]
fn drops_are_reported_at_most_once_per_flush_interval() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        channel: 4,
        flush_every: Duration::from_secs(30),
        ..opts(&c)
    };
    let j = fin_journal::Journal::open(&events(&dir), diagnostics(), o);
    for _ in 0..3 {
        assert!(j.pause_writer());
        for i in 0..10 {
            j.record(Source::FinNode, None, commit(i));
        }
        j.resume_writer();
        j.flush().unwrap();
    }
    let all = raw_lines(&events(&dir));
    assert_eq!(of_kind(&all, "journal.dropped").len(), 1);
    // Close reports what is left.
    j.close();
    let all = raw_lines(&events(&dir));
    assert_eq!(of_kind(&all, "journal.dropped").len(), 2);
    assert_eq!(
        of_kind(&all, "log.commit").len() as u64 + reported_drops(&all),
        30
    );
}

#[test]
fn a_due_retry_recovers_without_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    std::fs::write(events(&dir), b"in the way").unwrap();
    let j = fin_journal::Journal::open(&events(&dir), diagnostics(), opts(&c));
    assert_eq!(j.status().state, JournalState::Suspended);
    std::fs::remove_file(events(&dir)).unwrap();
    c.advance(Duration::from_secs(61));
    assert_eq!(j.status().state, JournalState::On);
    assert!(!events(&dir).exists(), "a retry creates nothing");
}

#[test]
fn suspension_subtracts_the_discarded_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        flush_every: Duration::from_secs(3600),
        flush_bytes: 1 << 20,
        ..opts(&c)
    };
    let j = fin_journal::Journal::open(&events(&dir), diagnostics(), o);
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    let on_disk = j.status_now().bytes;
    j.faults().unwrap().fail_writes(true);
    for i in 0..10 {
        j.record(Source::FinNode, None, commit(i));
    }
    assert!(j.flush().is_err());
    let s = j.status_now();
    assert_eq!(s.state, JournalState::Suspended);
    assert_eq!(s.bytes, on_disk, "the discarded lines are not counted");
    assert!(s.dropped >= 10);
}
