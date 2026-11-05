//! Store integration (plan Task 11): open, tick, read-only states and following, with two or
//! more servers (one server-data dir each) on one project folder.

mod integration_support;

use std::fs;

use fin_store::lease::LeaseState;
use fin_store::{MAIN, ReadOnlyReason, Store, StoreEvent};
use integration_support::*;

#[test]
fn writer_and_follower_converge() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    // B has no database: it builds one from the project file's genesis.
    let mut b = e.open("b");
    assert_eq!(b.head(MAIN), 0);
    assert_eq!(b.read_only(), Some(ReadOnlyReason::LeaseHeld));
    assert_eq!(e.flush(&mut a), 1);

    let r = e.poll(&mut b);
    let applied: Vec<&str> = r
        .events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Applied(c) => Some(c.tx.tx_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(applied, ["t1", "t2"]);
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    let st = b.status();
    assert_eq!(
        (st.branches[0].head, st.branches[0].exported_through),
        (2, 2)
    );
    assert!(matches!(
        st.lease,
        LeaseState::HeldElsewhere { epoch: 1, .. }
    ));

    // Following: later edits arrive too.
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
}

#[test]
fn follower_commit_is_read_only_lease_held() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    assert_eq!(b.head(MAIN), 1, "open imports");
    assert_eq!(refused(&mut b, "b1"), "LEASE_HELD");
    assert_eq!(b.head(MAIN), 1);
    // A retry of a committed tx is answered from the database.
    let t1 = a.ops_since(MAIN, 0).unwrap().remove(0);
    let again = try_commit(&mut b, "t1", vec![asset("/m/a.mov")]).unwrap();
    assert_eq!(again, t1);
    // Nothing of B's reaches the folder.
    assert_eq!(e.flush(&mut b), 0);
    let st = b.status();
    let ro = st.read_only.unwrap();
    assert_eq!(ro.reason, ReadOnlyReason::LeaseHeld);
    assert!(!ro.fix.is_empty());
    let json = serde_json::to_value(b.status()).unwrap();
    assert_eq!(json["readOnly"]["reason"], "LEASE_HELD");
    assert_eq!(json["lease"]["state"], "heldElsewhere");
    assert_eq!(json["storage"], "local");
}

#[test]
fn two_server_datas_one_folder() {
    // Review Focus 3: two servers on one machine (two server-data dirs) on one folder.
    let e = env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    assert_ne!(a.server_id(), b.server_id());
    assert_eq!(held_epoch(&a), Some(1));
    assert_eq!(held_epoch(&b), None);
    let LeaseState::HeldElsewhere { holder, epoch, .. } = b.lease_state() else {
        panic!("{:?}", b.lease_state());
    };
    assert_eq!(epoch, 1);
    assert!(holder.contains("finisher"), "{holder}");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    assert_eq!(refused(&mut b, "b1"), "LEASE_HELD");
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    // Polls keep both where they are.
    for _ in 0..3 {
        e.advance(2);
        e.poll(&mut a);
        e.poll(&mut b);
    }
    assert_eq!((held_epoch(&a), held_epoch(&b)), (Some(1), None));
    assert_eq!(a.read_only(), None);
}

#[test]
fn open_export_follow_in_unicode_dir() {
    // Review Focus 1: Unicode and spaces in the folder; segment names stay ASCII.
    let e = env_named("Мой проект 1");
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/клип.mov")]);
    assert_eq!(e.flush(&mut a), 1);
    for (path, _, _) in segments(&e.dir) {
        assert!(path.file_name().unwrap().to_str().unwrap().is_ascii());
    }
    let mut b = e.open("b");
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
}

#[test]
fn conflict_copy_makes_read_only() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let (path, _, _) = segments(&e.dir).remove(0);
    let copy = path.with_file_name(format!(
        "{} (conflicted copy 2026-11-04).fseg",
        path.file_stem().unwrap().to_str().unwrap()
    ));
    fs::copy(&path, &copy).unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ConflictCopy));
    assert_eq!(refused(&mut a, "t2"), "CONFLICT_COPY");
    let st = a.status();
    assert_eq!(st.foreign.len(), 1, "{st:?}");
    assert!(st.foreign[0].contains("conflicted copy"));
    assert!(st.warnings.iter().any(|w| w.code == "FOREIGN_FILES"));
    // Moved out by hand: writable on the next poll.
    fs::remove_file(&copy).unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), None);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
}

#[test]
fn lease_json_conflict_copy_makes_read_only_on_sync() {
    // Spec §3.5: a sync conflict copy of lease.json is a suspected split brain.
    let e = env();
    let a = e.create("a");
    drop(a);
    edit_project_file(&e.dir, |f| {
        f.insert("storageOverride".into(), "sync:dropbox".into());
    });
    let mut a = e.open("a");
    assert!(a.storage_kind().is_sync());
    assert_eq!(a.read_only(), None, "this process re-takes its own lease");
    let lease = e.dir.join(".finisher").join("lease.json");
    fs::copy(
        &lease,
        e.dir
            .join(".finisher")
            .join("lease (Bob's conflicted copy 2026-11-04).json"),
    )
    .unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ConflictCopy));
    assert_eq!(refused(&mut a, "t1"), "CONFLICT_COPY");
    commit_is_not_exported(&e, &mut a);
}

/// Nothing pending reaches the folder while read-only.
fn commit_is_not_exported(e: &Env, s: &mut fin_store::Store) {
    let before = segments(&e.dir).len();
    e.flush(s);
    assert_eq!(segments(&e.dir).len(), before);
}

#[test]
fn newer_format_read_only() {
    // A segment that needs a newer Finisher.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let (_, _, seg) = segments(&e.dir).remove(0);
    let mut h = seg.header.clone();
    (h.first_seq, h.last_seq) = (2, 2);
    h.prev_segment_hash = Some(seg.hash.clone());
    // 4B reads formats 1 and 2: 3 is newer.
    (h.format_version, h.min_reader_version) = (3, 3);
    let (bytes, hash) = fin_store::segment::encode(&h, &[b"{\"future\":true}".as_slice()]);
    let name =
        fin_store::names::SegmentName::new(1, 2, 2, fin_store::names::hash12(&hash).unwrap())
            .unwrap();
    fs::write(seg_dir(&e.dir).join(name.to_string()), bytes).unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::NewerFormat));
    assert_eq!(refused(&mut a, "t2"), "NEWER_FORMAT");

    // A project file that needs a newer Finisher: read-only from open on.
    let e = env();
    drop(e.create("a"));
    edit_project_file(&e.dir, |f| {
        f.insert("minReaderVersion".into(), 99.into());
    });
    let mut a = e.open("a");
    assert_eq!(a.read_only(), Some(ReadOnlyReason::NewerFormat));
    assert_eq!(refused(&mut a, "t1"), "NEWER_FORMAT");
}

#[test]
fn restart_re_establishes_read_only_before_any_commit() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let (path, _, _) = segments(&e.dir).remove(0);
    fs::copy(&path, path.with_file_name("copy (1).fseg")).unwrap();
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    drop(a); // t2 unexported; no tick saw the copy
    let mut a = e.open("a");
    assert_eq!(
        a.read_only(),
        Some(ReadOnlyReason::ConflictCopy),
        "set by open, before any tick"
    );
    assert_eq!(refused(&mut a, "t3"), "CONFLICT_COPY");
    assert_eq!(a.close().unwrap().segments_written, 0, "t2 stays local");
}

#[test]
fn status_events_only_on_change() {
    let e = env();
    let mut a = e.create("a");
    let first = e.poll(&mut a).events;
    let st = match first.as_slice() {
        [StoreEvent::Status(st)] => st.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(st.lease, LeaseState::Held { epoch: 1 });
    assert_eq!(st.read_only, None);
    assert_eq!(st.warnings, [], "{:?}", st.warnings);
    assert!(e.poll(&mut a).events.is_empty(), "unchanged: no event");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.poll(&mut a);
    e.advance(3);
    let events = e.poll(&mut a).events;
    let [StoreEvent::Status(st)] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(st.branches[0].exported_through, 1);
    assert!(st.last_export_ms.is_some());
    assert!(e.poll(&mut a).events.is_empty());
}

#[test]
fn a_file_incomplete_for_long_on_a_local_root_is_a_warning() {
    let e = env();
    let mut a = e.create("a");
    let dir = seg_dir(&e.dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("00000001-000000000001-000000000001-abcdefabcdef.fseg"),
        br#"{"magic":"finisher-segment""#,
    )
    .unwrap();
    for _ in 0..fin_store::INCOMPLETE_POLLS {
        e.poll(&mut a);
    }
    assert!(
        !a.status()
            .warnings
            .iter()
            .any(|w| w.code == "INCOMPLETE_FILES")
    );
    e.poll(&mut a);
    let st = a.status();
    let w = st
        .warnings
        .iter()
        .find(|w| w.code == "INCOMPLETE_FILES")
        .expect("warned");
    assert!(w.message.contains("abcdefabcdef"), "{}", w.message);
}

// ---- Review of 546b7f6: which conflict copies make the project read-only ----

#[test]
fn a_probe_conflict_copy_is_only_listed() {
    let e = env();
    let mut a = e.create("a");
    let probe = format!("lease.probe-{}", a.server_id());
    fs::write(
        e.dir
            .join(".finisher")
            .join(format!("{probe} (conflicted copy 2026-11-04)")),
        b"x",
    )
    .unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), None);
    assert_eq!(a.status().foreign.len(), 1);
}

#[test]
fn a_conflicted_branch_directory_makes_read_only() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let seg = e.dir.join(".finisher").join("segments");
    copy_dir(
        &seg.join(MAIN),
        &seg.join("main (conflicted copy 2026-11-04)"),
    );
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ConflictCopy));
}

#[test]
fn a_conflict_copy_of_the_project_file_is_a_warning() {
    let e = env();
    let mut a = e.create("a");
    fs::copy(
        e.dir.join("finisher.project.json"),
        e.dir
            .join("finisher.project (Bob's conflicted copy 2026-11-04).json"),
    )
    .unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), None);
    assert!(
        a.status()
            .warnings
            .iter()
            .any(|w| w.code == "PROJECT_FILE_CONFLICT")
    );
}

#[test]
fn a_newer_project_file_is_noticed_at_the_next_poll() {
    let e = env();
    let mut a = e.create("a");
    edit_project_file(&e.dir, |f| {
        f.insert("minReaderVersion".into(), 99.into());
    });
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::NewerFormat));
    edit_project_file(&e.dir, |f| {
        f.insert("minReaderVersion".into(), 1.into());
    });
    e.poll(&mut a);
    assert_eq!(a.read_only(), None);
}

#[test]
fn a_visible_file_under_our_segment_name_makes_read_only() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let planned = a.planned_segment_name(MAIN).unwrap().unwrap();
    let (_, _, ours) = {
        // Another server's valid segment, written under our planned name.
        let mut h = fin_store::segment::Header {
            format_version: 1,
            min_reader_version: 1,
            project_id: a.project_id().into(),
            branch: MAIN.into(),
            writer_id: "someoneelse".into(),
            epoch: 1,
            first_seq: 1,
            last_seq: 1,
            prev_segment_hash: None,
            op_schema_versions: [1, 1],
            created_at: 0,
            genesis_hash: a.genesis_hash(),
            branch_record: None,
            pack: false,
            replaces: vec![],
        };
        h.created_at = 7;
        let (bytes, _) =
            fin_store::segment::encode(&h, &[b"{\"txId\":\"x\",\"seq\":1}".as_slice()]);
        fs::create_dir_all(seg_dir(&e.dir)).unwrap();
        fs::write(seg_dir(&e.dir).join(&planned), &bytes).unwrap();
        ((), (), bytes)
    };
    e.flush(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ConflictCopy));
    assert_eq!(
        fs::read(seg_dir(&e.dir).join(&planned)).unwrap(),
        ours,
        "untouched"
    );
    assert_eq!(
        fs::read_dir(seg_dir(&e.dir)).unwrap().count(),
        1,
        "nothing exported"
    );
}

/// The args hash of an `--idem-key` commit travels with its record: another server that
/// imported it (or rebuilt its database from the folder) refuses the same key with other args.
#[test]
fn idem_args_hash_is_checked_on_other_servers() {
    let hash = |c: char| Some(format!("blake3:{}", c.to_string().repeat(64)));
    let tx = |s: &Store, h: Option<String>| fin_core::Tx {
        tx_id: "key-1".into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: s.head(MAIN),
        schema_version: fin_core::SCHEMA_VERSION,
        ops: vec![asset("/m/a.mov")],
        args_hash: h,
    };
    let e = env();
    let mut a = e.create("a");
    let first = a.commit(tx(&a, hash('a'))).unwrap();
    assert_eq!(e.flush(&mut a), 1);
    let mut b = e.open("b");
    e.poll(&mut b);
    assert_eq!(b.head(MAIN), 1);
    let reused = fin_core::CoreError::from(b.commit(tx(&b, hash('b'))).unwrap_err());
    assert_eq!(reused.code, "IDEM_KEY_REUSED", "{reused:?}");
    assert_eq!(b.commit(tx(&b, hash('a'))).unwrap(), first);

    // B's database rebuilt from the folder.
    let id = b.project_id().to_string();
    b.close().unwrap();
    fs::remove_file(fin_store::db_path(&e.data("b"), &id)).unwrap();
    let mut b = e.open("b");
    assert_eq!(b.head(MAIN), 1);
    let reused = fin_core::CoreError::from(b.commit(tx(&b, hash('b'))).unwrap_err());
    assert_eq!(reused.code, "IDEM_KEY_REUSED", "{reused:?}");
}
