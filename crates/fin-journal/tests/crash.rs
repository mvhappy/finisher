//! Crash safety (plan D, Decision 17): a child killed mid-write (`SIGKILL` on unix,
//! `TerminateProcess` on Windows) leaves, after the repair at open, only whole lines that
//! parse, in every file. Half the runs tear a write first, so a repair is sure to happen.

mod support;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fin_journal::{Filter, Journal};
use support::crash_child::{ENV, TEAR, ready, run, torn};
use support::*;

/// Runs as the child when `crash.rs` re-executes itself with [`ENV`] set; otherwise passes.
#[test]
fn crash_child() {
    if let Ok(dir) = std::env::var(ENV) {
        run(Path::new(&dir));
    }
}

fn random_ms(lo: u64, hi: u64) -> u64 {
    lo + RandomState::new().build_hasher().finish() % (hi - lo)
}

fn wait_for(p: &Path) {
    let t = Instant::now();
    while !p.exists() {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "no {p:?} from the child"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn kill_mid_write_leaves_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let ev = events(&dir);
    for run in 0..10 {
        let tear = run % 2 == 1;
        let _ = std::fs::remove_file(ready(&ev));
        let _ = std::fs::remove_file(torn(&ev));
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["crash_child", "--exact", "--test-threads", "1"])
            .env(ENV, &ev)
            .env_remove(TEAR)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if tear {
            cmd.env(TEAR, "1");
        }
        let mut child = cmd.spawn().unwrap();
        wait_for(&ready(&ev));
        if tear {
            wait_for(&torn(&ev));
        } else {
            std::thread::sleep(Duration::from_millis(random_ms(20, 300)));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        if tear {
            let newest = files(&ev).pop().unwrap();
            let bytes = std::fs::read(&newest).unwrap();
            assert!(!bytes.ends_with(b"\n"), "run {run}: the write was not torn");
        }
        // The repair runs at open with a purpose on.
        let c = clock();
        let j = Journal::open(&ev, diagnostics(), opts(&c));
        j.flush().unwrap();
        if tear {
            assert_eq!(j.faults().unwrap().repairs(), 1, "run {run}: no repair");
        }
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
