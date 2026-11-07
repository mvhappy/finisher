//! Purge (plan D, Task D3; Decision 29): `all` (with the key, `state.json` and every blob),
//! `before <date>` and `project <id>`, the last rewriting files atomically: a kill during a
//! purge leaves each file as it was or as it will be, never a mix.

mod support;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fin_core::events::{CollectPolicy, Source};
use fin_journal::{Clock, Journal, JournalOptions, PurgeSelector};
use support::purge_child::{AT_RENAME, ENV, parked, run};
use support::*;

/// Runs as the child when `purge.rs` re-executes itself with [`ENV`] set; otherwise passes.
#[test]
fn purge_child() {
    if let Ok(dir) = std::env::var(ENV) {
        run(Path::new(&dir));
    }
}

fn project_of(e: &fin_core::events::Envelope) -> Option<&str> {
    e.ctx.as_ref()?.project.as_ref().map(|p| p.as_str())
}

#[test]
fn purge_all_removes_key_state_and_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), training(), opts(&c));
    let before = j.hash("someone").unwrap();
    let b = j
        .attach_blob(
            vec![1u8; 64].into(),
            fin_core::events::names::MEDIA_TYPES
                .get("image/png")
                .unwrap(),
        )
        .unwrap();
    j.record(Source::Agent, None, tool_result(Some(b)));
    for i in 0..10 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.flush().unwrap();
    for f in ["key", "state.json", "blobs"] {
        assert!(events(&dir).join(f).exists(), "{f} before the purge");
    }
    let r = j.purge(PurgeSelector::All).unwrap();
    assert_eq!((r.removed, r.files, r.blobs), (11, 1, 1));
    assert!(
        all_paths(&events(&dir)).is_empty(),
        "{:?}",
        all_paths(&events(&dir))
    );
    let s = j.status();
    assert_eq!((s.files, s.bytes, s.blobs), (0, 0, 0));
    // Still on: a new key, so old and new hashes do not link.
    let after = j.hash("someone").unwrap();
    assert_ne!(before, after);
    j.record(Source::FinNode, None, commit(99));
    j.flush().unwrap();
    assert!(events(&dir).join("key").exists());
    assert_eq!(read_all(&j).len(), 1);
}

#[test]
fn purge_before_date() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 1500,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), o);
    for i in 0..40 {
        j.record(Source::FinNode, None, commit(i));
        c.advance(Duration::from_secs(1));
    }
    j.flush().unwrap();
    let files_before = files(&events(&dir)).len();
    assert!(files_before > 3);
    // Halfway, inside a file: whole files before it go, the one across it is rewritten.
    let cut = T0 + 3_600_000 + 20_500;
    let r = j.purge(PurgeSelector::Before(cut)).unwrap();
    assert_eq!(r.removed, 21);
    let micros: Vec<u64> = read_all(&j)
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, (21..40).collect::<Vec<_>>());
    assert!(files(&events(&dir)).len() < files_before);
    assert_eq!(raw_lines(&events(&dir)).len(), 19, "whole lines");
    // The journal keeps writing after it.
    j.record(Source::FinNode, None, commit(100));
    assert_eq!(read_all(&j).len(), 20);
    let _ = c.now_ms();
}

/// A journal of 60 entries over several files, P1 in every third; returns each file's bytes.
fn mixed(
    ev: &Path,
    c: &std::sync::Arc<fin_journal::ManualClock>,
) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let o = JournalOptions {
        max_file_bytes: 2048,
        ..opts(c)
    };
    let j = Journal::open(ev, diagnostics(), o);
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    j.set_project_policy(P2, Some(CollectPolicy::Default));
    for i in 0..60 {
        let p = if i % 3 == 0 { P1 } else { P2 };
        j.record(Source::FinNode, in_project(p), commit(i));
    }
    j.close();
    files(ev)
        .into_iter()
        .map(|f| {
            let b = std::fs::read(&f).unwrap();
            (f, b)
        })
        .collect()
}

fn kill_child(ev: &Path, at_rename: bool) {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["purge_child", "--exact", "--test-threads", "1"])
        .env(ENV, ev)
        .env_remove(AT_RENAME)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if at_rename {
        cmd.env(AT_RENAME, "1");
    }
    let mut child = cmd.spawn().unwrap();
    let t = Instant::now();
    while !parked(ev).exists() {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the child never parked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn purge_kill_between_temp_and_rename_keeps_the_old_file() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    let c = clock();
    let before = mixed(&ev, &c);
    kill_child(&ev, true);
    let temps: Vec<_> = all_paths(&ev)
        .into_iter()
        .filter(|p| p.to_str().unwrap().ends_with(".tmp"))
        .collect();
    assert_eq!(temps.len(), 1, "the temp file was written: {temps:?}");
    for (f, bytes) in &before {
        assert_eq!(
            &std::fs::read(f).unwrap(),
            bytes,
            "{f:?} changed before its rename"
        );
    }
    // The next open removes the temp file; the purge then completes.
    let j = Journal::open(&ev, diagnostics(), opts(&c));
    j.flush().unwrap();
    assert!(
        all_paths(&ev)
            .iter()
            .all(|p| !p.to_str().unwrap().ends_with(".tmp"))
    );
    j.purge(PurgeSelector::Project(P1.into())).unwrap();
    let all = read_all(&j);
    assert_eq!(all.len(), 40);
    assert!(all.iter().all(|e| project_of(e) == Some(P2)));
}

#[test]
fn failed_never_purge_is_retried() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    let c = clock();
    mixed(&ev, &c);
    let j = Journal::open(&ev, diagnostics(), opts(&c));
    j.flush().unwrap();
    j.faults().unwrap().fail_purge_writes(1);
    j.set_project_policy(P1, Some(CollectPolicy::Never));
    assert!(
        j.flush().is_err(),
        "the purge failed and suspended the journal"
    );
    assert!(
        read_all(&j).iter().any(|e| project_of(e) == Some(P1)),
        "not purged yet"
    );
    // The retry comes with the suspension's: the purge runs again and completes.
    c.advance(Duration::from_secs(61));
    j.flush().unwrap();
    let all = read_all(&j);
    assert_eq!(all.len(), 40, "every P1 entry gone");
    assert!(all.iter().all(|e| project_of(e) == Some(P2)));
    assert_eq!(j.status().state, fin_journal::JournalState::On);
}

#[test]
fn purge_all_then_flush_keeps_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    let c = clock();
    let o = JournalOptions {
        flush_bytes: 1,
        ..opts(&c)
    };
    let j = Journal::open(&ev, diagnostics(), o.clone());
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    // An entry queued before the purge is written (and flushed, writing the caller's new
    // key) before the purge deletes `events/key`.
    assert!(j.pause_writer());
    j.record(Source::FinNode, None, commit(2));
    let purge = {
        let j = j.clone();
        std::thread::spawn(move || j.purge(PurgeSelector::All))
    };
    std::thread::sleep(Duration::from_millis(50));
    j.resume_writer();
    purge.join().unwrap().unwrap();
    j.record(Source::FinNode, None, commit(3));
    j.flush().unwrap();
    assert!(
        ev.join("key").exists(),
        "the key is written again before the next line"
    );
    let h = j.hash("someone").unwrap();
    j.close();
    let j = Journal::open(&ev, diagnostics(), o);
    assert_eq!(
        j.hash("someone").unwrap(),
        h,
        "the key on disk is the one in use"
    );
}

#[test]
fn purge_project_rewrites_files_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    let c = clock();
    let before = mixed(&ev, &c);
    assert!(before.len() > 4);
    kill_child(&ev, false);
    // Each file is the old one or the rewritten one, whole.
    let (mut old, mut new) = (0, 0);
    for (f, bytes) in &before {
        let now = std::fs::read(f).unwrap();
        if &now == bytes {
            old += 1;
            continue;
        }
        let want: Vec<u8> = String::from_utf8(bytes.clone())
            .unwrap()
            .split_inclusive('\n')
            .filter(|l| !l.contains(P1))
            .collect::<String>()
            .into_bytes();
        assert_eq!(now, want, "{f:?} is a mix");
        new += 1;
    }
    assert_eq!(new, 2, "two rewrites before the kill");
    assert!(old > 0);
    // Reopened, the purge runs again to the end; no temp file is left.
    let j = Journal::open(&ev, diagnostics(), opts(&c));
    let r = j.purge(PurgeSelector::Project(P1.into())).unwrap();
    assert!(r.removed > 0);
    let all = read_all(&j);
    assert_eq!(all.len(), 40);
    assert!(all.iter().all(|e| project_of(e) == Some(P2)));
    let names: Vec<_> = all_paths(&ev)
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
}
