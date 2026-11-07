//! Rotation and the caps (plan D, Decision 16): a file rotates at its size limit or at the
//! first entry of a new UTC day; the age and size caps delete whole files, oldest first, at
//! rotation, hourly and at open, so they hold across restarts.

mod support;

use fin_core::events::Source;
use fin_journal::{Consent, Journal, JournalOptions};
use support::*;

fn names(dir: &std::path::Path) -> Vec<String> {
    files(dir)
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
        .collect()
}

fn micros(j: &Journal) -> Vec<u64> {
    of_kind(&read_all(j), "log.commit")
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect()
}

#[test]
fn rotates_at_size_and_at_utc_day() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 4096,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    for i in 0..60 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.flush().unwrap();
    let day0 = names(&events(&dir));
    assert!(day0.len() >= 3, "{day0:?}");
    for (n, p) in day0.iter().zip(files(&events(&dir))) {
        assert!(n.starts_with("j-20261107-") && n.ends_with(".jsonl"), "{n}");
        assert!(std::fs::metadata(&p).unwrap().len() <= 4096);
    }
    let mut sorted = day0.clone();
    sorted.sort();
    assert_eq!(sorted, day0, "names sort in write order");

    // 23:59:59.999 is still the same day; the next millisecond rotates.
    c.set(T0 + DAY.as_millis() as u64 - 1);
    j.record(Source::FinNode, None, commit(100));
    j.flush().unwrap();
    assert_eq!(names(&events(&dir)).len(), day0.len());
    c.set(T0 + DAY.as_millis() as u64);
    j.record(Source::FinNode, None, commit(101));
    j.flush().unwrap();
    let all = names(&events(&dir));
    assert_eq!(all.len(), day0.len() + 1);
    assert!(all.last().unwrap().starts_with("j-20261108-"));
    assert_eq!(micros(&j), (0..60).chain([100, 101]).collect::<Vec<_>>());
}

#[test]
fn age_and_size_caps_delete_oldest() {
    // Age: entries on four days with a 2-day cap; the first day's file goes at the rotation
    // into day 3.
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_age: 2 * DAY,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    for d in 0..4u64 {
        c.set(T0 + d * DAY.as_millis() as u64 + 1000);
        j.record(Source::FinNode, None, commit(d));
        j.flush().unwrap();
    }
    assert_eq!(micros(&j), [1, 2, 3]);
    let n = names(&events(&dir));
    assert!(n[0].starts_with("j-20261108-"), "{n:?}");
    // Hourly: at day 4 noon days 1 and 2 are past the cap; the hourly pass removes them
    // without a rotation, and day 3 stays.
    c.set(T0 + 4 * DAY.as_millis() as u64 + 12 * 3_600_000);
    j.flush().unwrap();
    assert_eq!(micros(&j), [3]);
    j.record(Source::FinNode, None, commit(9));
    assert_eq!(micros(&j), [3, 9]);

    // Size: 2 KiB files, an 8 KiB cap; the oldest files go first and the total holds.
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 8192,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    for i in 0..200 {
        j.record(Source::FinNode, None, commit(i));
        if i % 10 == 0 {
            j.flush().unwrap();
        }
    }
    j.flush().unwrap();
    let total: u64 = files(&events(&dir))
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= 8192 + 2048, "{total}");
    let kept = micros(&j);
    assert_eq!(*kept.last().unwrap(), 199);
    assert!(kept[0] > 100, "the oldest entries went first: {}", kept[0]);
    assert!(
        kept.windows(2).all(|w| w[1] == w[0] + 1),
        "whole files, no gaps"
    );
    assert!(j.status().bytes <= 8192 + 2048);
}

#[test]
fn caps_hold_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o.clone());
    for i in 0..100 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.close();
    let before = files(&events(&dir)).len();
    assert!(before > 4);

    // A smaller size cap applies at open, before anything is recorded.
    let small = JournalOptions {
        max_total_bytes: 4096,
        ..o.clone()
    };
    let j = Journal::open(&events(&dir), diagnostics(), small);
    j.flush().unwrap();
    let total: u64 = files(&events(&dir))
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= 4096, "{total}");
    assert_eq!(*micros(&j).last().unwrap(), 99);
    j.close();

    // Opened off a year later, retention still holds: the age cap deletes every file, and
    // nothing is created.
    c.advance(DAY * 400);
    let off = Journal::open(&events(&dir), Consent::OFF, o.clone());
    off.flush().unwrap();
    assert!(files(&events(&dir)).is_empty());
    assert_eq!(off.status_now().files, 0);
    off.close();
    let mut left: Vec<String> = std::fs::read_dir(events(&dir))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["key", "state.json"], "nothing new while off");
}

#[test]
fn size_cap_applies_while_off() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o.clone());
    for i in 0..100 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.close();
    let small = JournalOptions {
        max_total_bytes: 4096,
        ..o
    };
    let off = Journal::open(&events(&dir), Consent::OFF, small);
    off.flush().unwrap();
    let total: u64 = files(&events(&dir))
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert!(total <= 4096, "{total}");
    assert!(!files(&events(&dir)).is_empty(), "the newest files stay");
}
