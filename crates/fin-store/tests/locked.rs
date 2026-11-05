//! Files under `.finisher/` that cannot be read right now (`NotArrivedWhy::Locked`: held open
//! by another program, or deleted by compaction while another SMB client still holds them,
//! spec §3.13 items 13 and 15). Briefly locked they are quiet; after 60 s unchanged they are
//! reported, whatever the folder kind. A segment a store has already read is served from its
//! scan cache, so these tests lock files a follower has not read yet.

mod integration_support;

use std::fs;
use std::path::PathBuf;

use fin_store::conflict_names::Provider;
use fin_store::scan::lock_for_test;
use fin_store::storage_kind::StorageKind;
use fin_store::{Store, VerifyReport};
use integration_support::*;

fn warned(s: &Store, code: &str) -> Option<String> {
    s.status()
        .warnings
        .into_iter()
        .find(|w| w.code == code)
        .map(|w| w.message)
}

fn checksums(v: &VerifyReport) -> &fin_store::Check {
    v.checks
        .iter()
        .find(|c| c.name == "segment_checksums")
        .unwrap()
}

/// A project with three one-record segments; the first of them.
fn three_segments(e: &Env) -> (Store, PathBuf) {
    let mut a = e.create("a");
    for i in 0..3 {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        assert_eq!(e.flush(&mut a), 1);
    }
    let mut segs: Vec<PathBuf> = fs::read_dir(seg_dir(&e.dir))
        .unwrap()
        .map(|d| d.unwrap().path())
        .collect();
    segs.sort();
    (a, segs[0].clone())
}

#[test]
fn a_briefly_locked_segment_is_quiet_and_a_long_locked_one_is_reported() {
    let e = env();
    let (mut a, held) = three_segments(&e);
    let name = held.file_name().unwrap().to_string_lossy().into_owned();
    lock_for_test(&held, true);
    let mut b = e.open("b");
    // More polls than INCOMPLETE_POLLS, but well under 60 s: nothing to report.
    for _ in 0..fin_store::INCOMPLETE_POLLS + 3 {
        e.advance(1);
        e.poll(&mut b);
    }
    assert_eq!(warned(&b, "INCOMPLETE_FILES"), None);
    assert_eq!(warned(&b, "LOCKED_FILES"), None);
    assert_eq!(
        b.head(fin_store::MAIN),
        0,
        "waits for the locked first segment"
    );
    let v = b.verify();
    let c = checksums(&v);
    assert!(c.ok, "{c:?}");
    assert_eq!(c.details["unsettled"][0]["code"], "SEGMENT_LOCKED", "{c:?}");
    assert_eq!(c.details["unsettled"][0]["reason"], "locked", "{c:?}");
    // The holder's compaction reads afresh, and waits for it.
    a.set_compaction_grace_for_test(std::time::Duration::ZERO);
    let r = a.compact(true).unwrap();
    assert!(r.manifests.is_empty(), "{r:?}");
    assert!(
        r.waiting
            .iter()
            .any(|(_, why)| why.contains(&name) && why.contains("not readable yet")),
        "{r:?}"
    );

    // Unchanged and unreadable for 60 s: reported, on a local folder as on any.
    e.advance(55);
    e.poll(&mut b);
    let w = warned(&b, "LOCKED_FILES").expect("warned after 60 s");
    assert!(w.contains(&name), "{w}");
    assert_eq!(warned(&b, "INCOMPLETE_FILES"), None);
    let v = b.verify();
    let c = checksums(&v);
    assert!(!c.ok, "{c:?}");
    assert_eq!(c.details["unsettled"], serde_json::json!([]), "{c:?}");
    assert!(
        c.details["unreadable"][0]["path"]
            .as_str()
            .unwrap()
            .contains(&name),
        "{c:?}"
    );
    assert!(!v.ok);

    // Readable again: forgotten, and followed.
    lock_for_test(&held, false);
    e.poll(&mut b);
    assert_eq!(warned(&b, "LOCKED_FILES"), None);
    assert_eq!(b.head(fin_store::MAIN), 3);
    assert!(b.verify().ok);
}

#[test]
fn a_locked_segment_is_arriving_on_a_sync_folder() {
    let e = env();
    let (_a, held) = three_segments(&e);
    lock_for_test(&held, true);
    let mut b = e.open("b");
    b.set_test_storage_kind(StorageKind::Sync(Provider::Dropbox));
    e.poll(&mut b);
    let w = warned(&b, "SYNC_ARRIVING").expect("arriving");
    assert!(w.starts_with("1 segment file(s)"), "{w}");
    lock_for_test(&held, false);
    e.poll(&mut b);
    assert_eq!(warned(&b, "SYNC_ARRIVING"), None);
}

/// A damaged file that is readable on one pass and locked on the next (same length and
/// mtime) keeps the time it was first seen: the 60 s do not start again at every flip.
#[test]
fn a_file_flipping_between_incomplete_and_locked_keeps_its_time() {
    let e = env();
    let mut a = e.create("a");
    let dir = seg_dir(&e.dir);
    fs::create_dir_all(&dir).unwrap();
    let cut = dir.join("00000001-000000000001-000000000001-abcdefabcdef.fseg");
    fs::write(&cut, br#"{"magic":"finisher-segment""#).unwrap();
    lock_for_test(&cut, true);
    e.poll(&mut a);
    e.advance(35);
    e.poll(&mut a);
    lock_for_test(&cut, false);
    e.poll(&mut a); // incomplete this time
    lock_for_test(&cut, true);
    e.advance(30);
    e.poll(&mut a);
    let w = warned(&a, "LOCKED_FILES").expect("65 s since first seen");
    assert!(w.contains("abcdefabcdef"), "{w}");
    lock_for_test(&cut, false);
}
