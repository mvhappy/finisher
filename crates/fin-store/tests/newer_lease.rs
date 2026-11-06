//! A project that needs a newer Finisher is followed without the lease (`crate::newer`): no
//! acquisition, renewal, takeover or probe touches `.finisher/`, and a lease held when the
//! project turns newer is let go.

mod integration_support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CoreError, SCHEMA_VERSION};
use fin_store::ReadOnlyReason;
use integration_support::*;

/// Every file under `.finisher/` (segments included), by relative path, with its bytes.
fn dot_files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in fs::read_dir(at).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().into(), fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    let dot = dir.join(".finisher");
    walk(&dot, &dot, &mut out);
    out
}

fn names(files: &BTreeMap<PathBuf, Vec<u8>>) -> Vec<String> {
    files
        .keys()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn bump_schema(dir: &Path, v: u32) {
    edit_project_file(dir, |f| {
        f.insert("schemaVersion".into(), v.into());
    });
}

fn refused_takeover(e: CoreError) {
    assert_eq!(e.code, "LEASE_TAKEOVER_REFUSED", "{e:?}");
    assert_eq!(e.details["reason"], "NEWER_FORMAT");
    assert!(
        e.fix
            .as_deref()
            .is_some_and(|f| f.contains("update Finisher"))
    );
}

/// A newer project file: the open, many polls past the staleness window (the server's own
/// stopped lease, which it would re-take at once, and another server's, which it would take
/// over) and takeovers leave `.finisher/` byte-identical: no claim, no `lease.json` write, no
/// probe file.
#[test]
fn newer_project_file_never_touches_the_lease() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    drop(a);
    bump_schema(&e.dir, SCHEMA_VERSION + 1);
    let before = dot_files(&e.dir);
    assert!(
        names(&before).iter().any(|n| n == "lease.json"),
        "{:?}",
        names(&before)
    );

    for server in ["a", "b"] {
        let mut s = e.open(server);
        assert_eq!(s.read_only(), Some(ReadOnlyReason::NewerFormat), "{server}");
        assert_eq!(held_epoch(&s), None, "{server}");
        for _ in 0..4 {
            e.advance(20);
            e.poll(&mut s);
            assert_eq!(held_epoch(&s), None, "{server}");
        }
        refused_takeover(CoreError::from(s.takeover(false).unwrap_err()));
        refused_takeover(CoreError::from(s.takeover(true).unwrap_err()));
        assert_eq!(refused(&mut s, "t2"), "NEWER_FORMAT");
        assert_eq!(s.snapshot(fin_store::MAIN).unwrap().0, 1, "reads work");
        drop(s);
        let after = dot_files(&e.dir);
        assert_eq!(names(&after), names(&before), "{server}: no file added");
        assert!(after == before, "{server}: .finisher/ is byte-identical");
    }
}

/// The project file turns newer while this server holds the lease: the next poll lets it go
/// (no more heartbeats or probes), and once the file is back to our version the server
/// acquires again, at a fresh epoch.
#[test]
fn project_file_turning_newer_lets_go_of_the_lease() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let epoch = held_epoch(&a).unwrap();
    bump_schema(&e.dir, SCHEMA_VERSION + 1);
    e.poll(&mut a);
    assert_eq!(held_epoch(&a), None);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::NewerFormat));
    // The heartbeat has stopped: nothing under `.finisher/` changes from here on.
    let let_go = dot_files(&e.dir);
    for _ in 0..4 {
        e.advance(20);
        e.poll(&mut a);
    }
    assert!(dot_files(&e.dir) == let_go, "no heartbeat, claim or probe");
    refused_takeover(CoreError::from(a.takeover(true).unwrap_err()));

    bump_schema(&e.dir, SCHEMA_VERSION);
    e.poll(&mut a);
    assert!(
        held_epoch(&a).is_some_and(|n| n > epoch),
        "re-acquired at a fresh epoch"
    );
    assert_eq!(a.read_only(), None);
}

/// A segment that needs a newer Finisher arrives while this server holds the lease: the
/// import pass lets the lease go, and later polls neither renew nor retry it.
#[test]
fn newer_segment_lets_go_of_the_lease() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    assert!(held_epoch(&a).is_some());
    let (_, _, seg) = segments(&e.dir).remove(0);
    let mut h = seg.header.clone();
    (h.first_seq, h.last_seq) = (2, 2);
    h.prev_segment_hash = Some(seg.hash.clone());
    (h.format_version, h.min_reader_version) = (3, 3);
    let (bytes, hash) = fin_store::segment::encode(&h, &[b"{\"future\":true}".as_slice()]);
    let name =
        fin_store::names::SegmentName::new(1, 2, 2, fin_store::names::hash12(&hash).unwrap())
            .unwrap();
    fs::write(seg_dir(&e.dir).join(name.to_string()), bytes).unwrap();
    e.poll(&mut a);
    assert_eq!(held_epoch(&a), None);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::NewerFormat));
    let let_go = dot_files(&e.dir);
    for _ in 0..4 {
        e.advance(20);
        e.poll(&mut a);
        assert_eq!(held_epoch(&a), None);
    }
    assert!(dot_files(&e.dir) == let_go, "no heartbeat, claim or probe");
    refused_takeover(CoreError::from(a.takeover(true).unwrap_err()));
}
