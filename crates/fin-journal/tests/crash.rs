//! Crash safety (plan D, Decision 17): a child killed mid-write (`SIGKILL` on unix,
//! `TerminateProcess` on Windows) leaves, after the repair at open, only whole lines that
//! parse, in every file.

mod support;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::process::{Command, Stdio};
use std::time::Duration;

use fin_journal::{Filter, Journal};
use support::crash_child::{ENV, run};
use support::*;

/// Runs as the child when `crash.rs` re-executes itself with [`ENV`] set; otherwise passes.
#[test]
fn crash_child() {
    if let Ok(dir) = std::env::var(ENV) {
        run(std::path::Path::new(&dir));
    }
}

fn random_ms(lo: u64, hi: u64) -> u64 {
    lo + RandomState::new().build_hasher().finish() % (hi - lo)
}

#[test]
fn kill_mid_write_leaves_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    for run in 0..10 {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["crash_child", "--exact", "--test-threads", "1"])
            .env(ENV, &ev)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(random_ms(80, 400)));
        child.kill().unwrap();
        child.wait().unwrap();
        // The repair runs at open with a purpose on.
        let c = clock();
        let j = Journal::open(&ev, diagnostics(), opts(&c));
        j.flush().unwrap();
        let mut it = j.read(&Filter::default());
        let n = it.by_ref().count();
        assert_eq!(it.skipped(), 0, "run {run}: a line that does not parse");
        assert!(n > 0, "run {run}: the child wrote nothing");
        for f in files(&ev) {
            let bytes = std::fs::read(&f).unwrap();
            assert!(
                bytes.is_empty() || bytes.ends_with(b"\n"),
                "run {run}: {f:?} torn"
            );
        }
        j.close();
    }
    // Every line on disk is a whole entry.
    assert!(!raw_lines(&ev).is_empty());
}
