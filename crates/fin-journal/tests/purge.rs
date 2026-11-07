//! Purge (plan D, Task D3; Decision 29): `all` (with the key, `state.json` and every blob),
//! `before <date>` and `project <id>`, the last rewriting files atomically: a kill during a
//! purge leaves each file as it was or as it will be, never a mix.

mod support;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fin_core::events::{CollectPolicy, Source};
use fin_journal::{Clock, Journal, JournalOptions, PurgeSelector};
use support::purge_child::{ENV, parked, run};
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

#[test]
fn purge_project_rewrites_files_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        ..opts(&c)
    };
    let j = Journal::open(&ev, diagnostics(), o);
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    j.set_project_policy(P2, Some(CollectPolicy::Default));
    for i in 0..60 {
        let p = if i % 3 == 0 { P1 } else { P2 };
        j.record(Source::FinNode, in_project(p), commit(i));
    }
    j.close();
    let before: Vec<(std::path::PathBuf, Vec<u8>)> = files(&ev)
        .into_iter()
        .map(|f| {
            let b = std::fs::read(&f).unwrap();
            (f, b)
        })
        .collect();
    assert!(before.len() > 4);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["purge_child", "--exact", "--test-threads", "1"])
        .env(ENV, &ev)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while !parked(&ev).exists() {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the child never parked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
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
