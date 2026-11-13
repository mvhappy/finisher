//! Links in place of `.finisher/` or its reserved directories are never followed (sub-project
//! 4 spec §3.6.1 "Never followed"): scans refuse them and nothing is published through them.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use fin_core::{Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::names::{self, SegmentName};
use fin_store::scan::{Entry, scan};
use fin_store::segment::{self, Header};
use fin_store::{MAIN, Store};

fn header(branch: &str, seq: u64) -> Header {
    Header {
        format_version: segment::FORMAT_VERSION,
        min_reader_version: segment::MIN_READER_VERSION,
        project_id: "p1".into(),
        branch: branch.into(),
        writer_id: "w1".into(),
        epoch: 1,
        first_seq: seq,
        last_seq: seq,
        prev_segment_hash: None,
        op_schema_versions: [1, 1],
        created_at: 0,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
        branch_record: None,
        pack: false,
        replaces: vec![],
    }
}

/// A genuine one-record segment in `<root>/.finisher/segments/main/`.
fn put_segment(root: &Path) -> PathBuf {
    let (bytes, hash) = segment::encode(&header(MAIN, 1), &[b"{\"seq\":1}"]);
    let name = SegmentName::new(1, 1, 1, names::hash12(&hash).unwrap()).unwrap();
    let dir = root.join(".finisher/segments").join(MAIN);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name.to_string());
    fs::write(&path, bytes).unwrap();
    path
}

fn no_segments(entries: &[Entry]) -> bool {
    !entries.iter().any(|e| matches!(e, Entry::Segment { .. }))
}

fn assert_refused(project: &Path) {
    match scan(project) {
        Ok(entries) => panic!("a linked reserved directory was scanned: {entries:?}"),
        Err(e) => assert!(
            e.to_string().contains("link"),
            "the error names the link: {e}"
        ),
    }
}

#[test]
fn scan_refuses_a_linked_finisher_dir() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    put_segment(outside.path());
    symlink(
        outside.path().join(".finisher"),
        project.path().join(".finisher"),
    )
    .unwrap();
    assert_refused(project.path());
}

#[test]
fn scan_refuses_each_linked_reserved_dir() {
    for sub in ["segments", "snapshots", "compaction"] {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        put_segment(outside.path());
        let target = outside.path().join(".finisher").join(sub);
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(project.path().join(".finisher")).unwrap();
        symlink(&target, project.path().join(".finisher").join(sub)).unwrap();
        assert_refused(project.path());
    }
}

#[test]
fn scan_never_enters_a_linked_branch_dir() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    put_segment(outside.path());
    let segs = project.path().join(".finisher/segments");
    fs::create_dir_all(&segs).unwrap();
    let link = segs.join(MAIN);
    symlink(outside.path().join(".finisher/segments").join(MAIN), &link).unwrap();
    let entries = scan(project.path()).unwrap();
    assert!(no_segments(&entries), "{entries:?}");
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Foreign { path, .. } if *path == link))
    );
}

fn commit(s: &mut Store, id: &str) {
    try_commit(s, id).unwrap();
}

fn try_commit(s: &mut Store, id: &str) -> Result<(), fin_store::StoreError> {
    let op: Op = fin_core::AssetAdd::media(
        "$a",
        "/media/a.mov",
        Rational::from_int(60),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
        }),
        None,
        "fp",
    )
    .into();
    let base_rev = s.head(MAIN);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops: vec![op],
    })
    .map(drop)
}

fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |rd| rd.flatten().count())
}

/// A project with one exported segment; returns `(tmp, data, project)`.
fn exported_project() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("server-data");
    let dir = tmp.path().join("project");
    let mut s = Store::create(&data, &dir, "Demo", Rational::from_int(30), None).unwrap();
    commit(&mut s, "a");
    let r = s.close().unwrap();
    assert_eq!((r.segments_written, r.error), (1, None));
    assert_eq!(count_files(&dir.join(".finisher/segments").join(MAIN)), 1);
    (tmp, data, dir)
}

/// Moves `<project>/<rel>` to an isolated directory and puts a link in its place; returns
/// the moved directory.
fn replace_with_link(project: &Path, rel: &str) -> (tempfile::TempDir, PathBuf) {
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("moved");
    fs::rename(project.join(rel), &moved).unwrap();
    symlink(&moved, project.join(rel)).unwrap();
    (outside, moved)
}

/// Reopens, commits another edit, then puts the link in place of `rel` before the export:
/// nothing may land beyond it.
fn assert_export_refused(rel: &str, outside_segments: impl Fn(&Path) -> PathBuf) {
    let (_tmp, data, dir) = exported_project();
    let mut s = Store::open(&data, &dir).unwrap();
    commit(&mut s, "b");
    let (_out, moved) = replace_with_link(&dir, rel);
    let r = s.close().unwrap();
    assert_eq!(r.segments_written, 0, "{r:?}");
    assert_eq!(
        r.error.map(|e| e.code),
        Some("PROJECT_DIR_UNWRITABLE".into())
    );
    assert_eq!(
        count_files(&outside_segments(&moved)),
        1,
        "the external directory grew"
    );
}

#[test]
fn export_never_writes_through_a_linked_finisher_dir() {
    assert_export_refused(".finisher", |m| m.join("segments").join(MAIN));
}

#[test]
fn export_never_writes_through_a_linked_segments_dir() {
    assert_export_refused(".finisher/segments", |m| m.join(MAIN));
}

#[test]
fn export_never_writes_through_a_linked_branch_dir() {
    assert_export_refused(".finisher/segments/main", Path::to_path_buf);
}

/// The review's reproduction: the segments directory is moved out and linked back, then the
/// project is reopened and edited. Nothing is read or written beyond the link.
#[test]
fn reopened_project_never_publishes_through_a_linked_segments_dir() {
    let (_tmp, data, dir) = exported_project();
    let (_out, moved) = replace_with_link(&dir, ".finisher/segments");
    let mut s = Store::open(&data, &dir).unwrap();
    // The import cannot read the folder, so the project stays read-only.
    let listed = s.import().unwrap().error.expect("the scan is refused");
    assert!(listed.message.contains("link"), "{listed:?}");
    assert!(try_commit(&mut s, "b").is_err());
    let r = s.close().unwrap();
    assert_eq!(r.segments_written, 0, "{r:?}");
    assert_eq!(
        count_files(&moved.join(MAIN)),
        1,
        "the external directory grew"
    );
}
