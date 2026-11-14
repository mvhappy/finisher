//! The daily backup off the store (`Store::take_due_backup`, `DailyBackup::run`): it runs
//! while the store commits, a cancel leaves nothing, and a crash (a killed process, a partial
//! file) never leaves a backup that is taken as valid.

mod integration_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fin_store::lease::MonoClock;
use fin_store::{BackupCancel, BackupOutcome, DailyBackup, Store, TickOptions};
use integration_support::*;
use rusqlite::{Connection, OpenFlags};

const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

/// The role of a re-run of this test binary (`a_killed_backup_leaves_no_daily_file`).
const CHILD: &str = "FIN_BACKUP_CRASH_CHILD";

fn due_backup(e: &Env, s: &mut Store) -> DailyBackup {
    s.poll_now_for_test();
    s.tick_with(e.clock.now(), LIGHT).unwrap();
    assert!(s.due_work().backup);
    s.take_due_backup().expect("today's backup is due")
}

fn backups_dir(s: &Store) -> PathBuf {
    s.db_path().parent().unwrap().join("backups")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn dailies(dir: &Path) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| n.starts_with("daily-"))
        .collect()
}

/// `PRAGMA integrity_check` of `path` and its committed transactions.
fn check(path: &Path) -> (String, i64) {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    let txs: i64 = c
        .query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))
        .unwrap();
    (ok, txs)
}

fn incarnation(path: &Path) -> String {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.query_row(
        "SELECT value FROM meta WHERE key = 'incarnation'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn fill(s: &mut Store, from: usize, n: usize) {
    for i in from..from + n {
        commit(s, &format!("t{i}"), vec![asset(&format!("/m/{i}.mov"))]);
    }
}

#[test]
fn the_daily_backup_runs_while_the_store_commits() {
    let e = env();
    let mut a = e.create("a");
    fill(&mut a, 0, 100);
    let job = due_backup(&e, &mut a);
    let cancel = Arc::new(BackupCancel::default());
    cancel.pause_for_test(400);
    let (j, c) = (job.clone(), cancel.clone());
    let running = std::thread::spawn(move || j.run(&c));
    let started = Instant::now();
    // 50 commits while it runs (the first ones well inside its pause).
    for i in 100..150 {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        if i == 100 {
            assert!(!running.is_finished(), "the backup is still running");
        }
    }
    let outcome = running.join().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300));
    let BackupOutcome::Taken(path) = &outcome else {
        panic!("{outcome:?}");
    };
    a.finish_backup(&job, &outcome);
    let (ok, txs) = check(path);
    assert_eq!(ok, "ok");
    // One snapshot: the 100 commits before it, and at most those committed meanwhile.
    assert!((100..=150).contains(&txs), "{txs}");
    assert_ne!(
        incarnation(path),
        a.incarnation(),
        "a backup is its own incarnation"
    );
    assert_eq!(dailies(&backups_dir(&a)).len(), 1);
    assert!(!names(&backups_dir(&a)).iter().any(|n| n.ends_with(".tmp")));
    a.poll_now_for_test();
    a.tick_with(e.clock.now(), LIGHT).unwrap();
    assert!(!a.due_work().backup, "taken today");
    assert!(a.take_due_backup().is_none());
}

#[test]
fn a_cancelled_backup_leaves_nothing_and_is_due_again() {
    let e = env();
    let mut a = e.create("a");
    fill(&mut a, 0, 20);
    let job = due_backup(&e, &mut a);
    let cancel = Arc::new(BackupCancel::default());
    cancel.pause_for_test(10_000);
    let (j, c) = (job.clone(), cancel.clone());
    let running = std::thread::spawn(move || j.run(&c));
    // The copy exists (a temp file) before the cancel.
    let dir = backups_dir(&a);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !names(&dir).iter().any(|n| n.ends_with(".tmp")) {
        assert!(Instant::now() < deadline, "no copy started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let asked = Instant::now();
    cancel.cancel();
    let outcome = running.join().unwrap();
    assert!(asked.elapsed() < Duration::from_secs(5));
    assert!(matches!(outcome, BackupOutcome::Cancelled), "{outcome:?}");
    a.finish_backup(&job, &outcome);
    assert_eq!(names(&dir), Vec::<String>::new(), "nothing left behind");
    // Cancelled is not failed: due again at the next poll, not after the retry wait.
    let again = due_backup(&e, &mut a);
    let outcome = again.run(&BackupCancel::default());
    assert!(matches!(outcome, BackupOutcome::Taken(_)), "{outcome:?}");
}

#[test]
fn a_cancel_before_the_start_writes_nothing() {
    let e = env();
    let mut a = e.create("a");
    let job = due_backup(&e, &mut a);
    let cancel = BackupCancel::default();
    cancel.cancel();
    assert!(matches!(job.run(&cancel), BackupOutcome::Cancelled));
    assert!(dailies(&backups_dir(&a)).is_empty());
}

#[test]
fn a_partial_daily_file_is_not_taken_as_the_days_backup() {
    let e = env();
    let mut a = e.create("a");
    fill(&mut a, 0, 30);
    // A whole backup, then the same day's file cut short (a crash that lost the data a
    // rename had published, or a copy by hand), next to a partial temp of a crashed copy.
    let job = due_backup(&e, &mut a);
    let BackupOutcome::Taken(path) = job.run(&BackupCancel::default()) else {
        panic!("not taken");
    };
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    let dir = backups_dir(&a);
    std::fs::write(dir.join(".~fin-crashed.tmp"), &bytes[..bytes.len() / 3]).unwrap();
    // Not reported back (`finish_backup`): the store finds the day's backup due again.
    let job = due_backup(&e, &mut a);
    let outcome = job.run(&BackupCancel::default());
    let BackupOutcome::Taken(again) = &outcome else {
        panic!("the partial file was taken as valid: {outcome:?}");
    };
    assert_eq!(again, &path);
    assert_eq!(check(again), ("ok".to_string(), 30));
    // The in-tick backup agrees: today has a whole backup now.
    assert_eq!(a.daily_backup(now_ms()).unwrap(), None);
    // A fresh temp is not stale yet; it is never a backup either way.
    assert_eq!(dailies(&dir).len(), 1);
}

fn now_ms() -> i64 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    i64::try_from(ms).unwrap()
}

/// Kills a process in the middle of its backup (its copy written, not yet verified or moved
/// into place): no `daily-*` file, and the next open takes a whole one.
#[test]
fn a_killed_backup_leaves_no_daily_file() {
    if let Ok(dirs) = std::env::var(CHILD) {
        // The child: open, start the backup and stall inside it until killed.
        let (data, project) = dirs.split_once('\n').unwrap();
        let mut s = Store::open(Path::new(data), Path::new(project)).unwrap();
        s.tick_with(Instant::now(), LIGHT).unwrap();
        let job = s.take_due_backup().unwrap();
        let cancel = BackupCancel::default();
        cancel.pause_for_test(600_000);
        job.run(&cancel);
        unreachable!("killed during its pause");
    }
    let e = env();
    let mut a = e.create("a");
    fill(&mut a, 0, 40);
    let dir = backups_dir(&a);
    a.close().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_killed_backup_leaves_no_daily_file",
            "--test-threads=1",
        ])
        .env(
            CHILD,
            format!("{}\n{}", e.data("a").display(), e.dir.display()),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        // The copy is written once the temp holds a whole database.
        let temp = names(&dir).into_iter().find(|n| n.ends_with(".tmp"));
        if temp.is_some_and(|t| check_whole(&dir.join(t))) {
            break;
        }
        assert!(child.try_wait().unwrap().is_none(), "the child ended early");
        assert!(Instant::now() < deadline, "the child never copied");
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(dailies(&dir).is_empty(), "{:?}", names(&dir));
    let mut b = e.open("a");
    let job = due_backup(&e, &mut b);
    let outcome = job.run(&BackupCancel::default());
    let BackupOutcome::Taken(path) = &outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(check(path), ("ok".to_string(), 40));
}

/// Whether the temp at `path` holds the 40 transactions (read as an immutable file: the
/// child still has it open).
fn check_whole(path: &Path) -> bool {
    let p = path.display().to_string().replace('\\', "/");
    let p = if p.as_bytes().get(1) == Some(&b':') {
        format!("/{p}")
    } else {
        p
    };
    let uri = format!("file:{p}?immutable=1");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let Ok(c) = Connection::open_with_flags(uri, flags) else {
        return false;
    };
    c.query_row("SELECT COUNT(*) FROM txs", [], |r| r.get::<_, i64>(0))
        .is_ok_and(|n| n == 40)
}
