//! Blobs (plan D, Task D3; Decisions 17 and 20): written before the line that references
//! them, only with training, counted per file, collected with their last reference by purge,
//! retention and `maxMiB`, and counted against `maxMiB`.

mod support;

use std::sync::Arc;
use std::time::Duration;

use fin_core::events::names::MEDIA_TYPES;
use fin_core::events::{BlobRef, CollectPolicy, Source};
use fin_journal::{Journal, JournalOptions, PurgeSelector};
use support::*;

fn bytes(n: usize, fill: u8) -> Arc<[u8]> {
    vec![fill; n].into()
}

fn attach(j: &Journal, n: usize, fill: u8) -> BlobRef {
    j.attach_blob(bytes(n, fill), MEDIA_TYPES.get("image/webp").unwrap())
        .expect("training is on")
}

fn hex(b: &BlobRef) -> String {
    b.hash.hex().to_owned()
}

fn blob_names(dir: &tempfile::TempDir) -> Vec<String> {
    blob_files(&events(dir))
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn blob_written_before_its_reference() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        flush_every: Duration::from_secs(3600),
        flush_bytes: 1 << 20,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    let b = attach(&j, 5000, 1);
    assert!(b.hash.as_str().starts_with("kb:"));
    assert_eq!(b.bytes, 5000);
    j.faults().unwrap().hold_writes(true);
    j.record(Source::Agent, None, tool_result(Some(b.clone())));
    let flush = {
        let j = j.clone();
        std::thread::spawn(move || j.flush())
    };
    // The writer has handled the entry and holds the flush: the blob is on disk first.
    assert!(j.faults().unwrap().wait_held(Duration::from_secs(5)));
    assert_eq!(blob_names(&dir), [hex(&b)]);
    assert!(
        raw_lines(&events(&dir)).is_empty(),
        "the line is not written yet"
    );
    j.faults().unwrap().hold_writes(false);
    flush.join().unwrap().unwrap();
    let all = read_all(&j);
    assert_eq!(all[0].payload["output"]["blob"]["hash"], b.hash.as_str());
    let on_disk = std::fs::read(events(&dir).join("blobs").join(hex(&b))).unwrap();
    assert_eq!(on_disk, vec![1u8; 5000]);
    assert_eq!((j.status().blobs, j.status().blob_bytes), (1, 5000));
    // A blob whose bytes never reached the writer takes the entry's content with it.
    let ghost = BlobRef {
        hash: fin_core::events::BlobHash::from_bytes(&[9u8; 32]),
        ..b
    };
    j.record(Source::Agent, None, tool_result(Some(ghost)));
    let all = read_all(&j);
    assert!(
        all[1].payload.get("output").is_none(),
        "no dangling reference"
    );
    assert_eq!(blob_names(&dir).len(), 1);
}

#[test]
fn blob_not_written_without_training() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    assert!(
        j.attach_blob(bytes(10, 1), MEDIA_TYPES.get("image/png").unwrap())
            .is_none()
    );
    j.close();
    // Attached under training, withdrawn before the entry is written: never written.
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::open(&events(&dir), training(), opts(&c));
    assert!(j.pause_writer());
    let b = attach(&j, 100, 2);
    j.record(Source::Agent, None, tool_result(Some(b)));
    j.set_gate_for_test(diagnostics());
    j.resume_writer();
    let all = read_all(&j);
    assert!(all[0].payload.get("output").is_none());
    assert!(blob_names(&dir).is_empty());
    // Too large, or not a media type: refused.
    let j = Journal::open(&events(&dir), training(), opts(&c));
    let big = bytes(fin_journal::MAX_BLOB_BYTES + 1, 0);
    assert!(
        j.attach_blob(big, MEDIA_TYPES.get("image/png").unwrap())
            .is_none()
    );
}

#[test]
fn blob_gc_on_purge_project_and_before() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), training(), opts(&c));
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    let a = attach(&j, 100, 1);
    j.record(Source::Agent, in_project(P1), tool_result(Some(a.clone())));
    let b = attach(&j, 100, 2);
    j.record(Source::Agent, None, tool_result(Some(b.clone())));
    j.flush().unwrap();
    assert_eq!(blob_names(&dir).len(), 2);
    let r = j.purge(PurgeSelector::Project(P1.to_uppercase())).unwrap();
    assert_eq!((r.removed, r.blobs), (1, 1));
    assert_eq!(blob_names(&dir), [hex(&b)]);
    c.advance(Duration::from_secs(10));
    let r = j.purge(PurgeSelector::Before(c_now(&c))).unwrap();
    assert_eq!((r.removed, r.blobs), (1, 1));
    assert!(blob_names(&dir).is_empty());
}

fn c_now(c: &fin_journal::ManualClock) -> u64 {
    use fin_journal::Clock;
    c.now_ms()
}

#[test]
fn blob_gc_on_retention_and_max_mib() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 1024,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o.clone());
    let old = attach(&j, 1000, 1);
    j.record(Source::Agent, None, tool_result(Some(old.clone())));
    j.flush().unwrap();
    // 31 days later the next entry starts a new file; the age cap takes the old one and
    // its blob.
    c.advance(DAY * 31);
    let new = attach(&j, 1000, 2);
    j.record(Source::Agent, None, tool_result(Some(new.clone())));
    j.flush().unwrap();
    assert_eq!(blob_names(&dir), [hex(&new)]);
    j.close();
    // Reopened with a cap below what the blobs need: the oldest closed files and their
    // blobs go.
    let j = Journal::open(&events(&dir), training(), o.clone());
    for i in 0..6 {
        let b = attach(&j, 40_000, 10 + i);
        j.record(Source::Agent, None, tool_result(Some(b)));
        j.flush().unwrap();
    }
    j.close();
    let small = JournalOptions {
        max_total_bytes: 100_000,
        ..o
    };
    let j = Journal::open(&events(&dir), training(), small);
    let s = j.status();
    assert!(s.bytes + s.blob_bytes <= 100_000, "{s:?}");
    assert!(s.blobs >= 1 && s.blobs < 7, "{s:?}");
    assert!(
        !blob_names(&dir).contains(&hex(&new)),
        "the oldest went first"
    );
}

#[test]
fn shared_blob_survives_until_last_reference() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 600,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    let b = attach(&j, 300, 3);
    j.record(Source::Agent, None, tool_result(Some(b.clone())));
    j.flush().unwrap();
    c.advance(Duration::from_secs(5));
    let cut = c_now(&c);
    // The same bytes again: the same address, written once.
    let again = attach(&j, 300, 3);
    assert_eq!(again, b);
    j.record(Source::Agent, None, tool_result(Some(again)));
    j.flush().unwrap();
    assert!(files(&events(&dir)).len() >= 2, "two files reference it");
    let r = j.purge(PurgeSelector::Before(cut)).unwrap();
    assert_eq!((r.removed, r.blobs), (1, 0));
    assert_eq!(blob_names(&dir), [hex(&b)], "one reference left");
    c.advance(Duration::from_secs(5));
    let r = j.purge(PurgeSelector::Before(c_now(&c))).unwrap();
    assert_eq!((r.removed, r.blobs), (1, 1));
    assert!(blob_names(&dir).is_empty());
}

#[test]
fn orphan_blob_from_crash_is_collected() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), training(), opts(&c));
    let kept = attach(&j, 64, 1);
    j.record(Source::Agent, None, tool_result(Some(kept.clone())));
    j.close();
    // A crash after a blob was written and before its line: no reference to it.
    let blobs = events(&dir).join("blobs");
    std::fs::write(blobs.join("ab".repeat(32)), b"orphan").unwrap();
    std::fs::write(blobs.join(".cd.tmp"), b"half").unwrap();
    let j = Journal::open(&events(&dir), training(), opts(&c));
    j.flush().unwrap();
    assert_eq!(blob_names(&dir), [hex(&kept)]);
    assert_eq!(j.status().blobs, 1);
}

#[test]
fn max_mib_counts_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 64 * 1024,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    for i in 0..20u8 {
        let b = attach(&j, 10_000, i);
        j.record(Source::Agent, None, tool_result(Some(b)));
        j.flush().unwrap();
    }
    let s = j.status();
    // The files alone are far under the cap: the blobs are what it removes. A file's blobs
    // count towards its rotation, so at most the open file's share is over.
    assert!(s.bytes < 16 * 1024, "{s:?}");
    assert!(s.bytes + s.blob_bytes <= 64 * 1024 + 10_000 + 2048, "{s:?}");
    assert!(s.blobs < 20, "{s:?}");
    let refs: Vec<String> = read_all(&j)
        .iter()
        .filter_map(|e| {
            e.payload["output"]["blob"]["hash"]
                .as_str()
                .map(|h| h[3..].to_owned())
        })
        .collect();
    let mut on_disk = blob_names(&dir);
    on_disk.sort();
    let mut refs_sorted = refs.clone();
    refs_sorted.sort();
    assert_eq!(
        on_disk, refs_sorted,
        "every kept reference resolves, nothing else"
    );
}
