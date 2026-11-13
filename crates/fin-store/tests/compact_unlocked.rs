//! The phased compaction step (`Store::tick_phased`, `Store::run_due_compaction_step_phased`):
//! its folder I/O (the `compaction/` listing, the fresh reads, the pack writes, the fences)
//! runs outside the store lock, so short calls answer meanwhile, and every delete is still
//! checked, with the lock, against the database as it is then.

mod integration_support;

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fin_store::lease::MonoClock;
use fin_store::{
    CompactReport, MAIN, Store, StoreAccess, StoreError, TickOptions, TickPoint, TickReport,
};
use integration_support::*;

const GRACE: Duration = Duration::from_secs(5);

/// Ticks that leave the heavy steps to the caller (as fin-node's do).
const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

/// A store behind a mutex, as fin-node keeps it; `None` once closed.
#[derive(Clone)]
struct Shared(Arc<Mutex<Option<Store>>>);

impl StoreAccess for Shared {
    fn with<R>(&mut self, f: impl FnOnce(&mut Store) -> R) -> Option<R> {
        self.0.lock().unwrap().as_mut().map(f)
    }
}

impl Shared {
    fn new(s: Store) -> Shared {
        Shared(Arc::new(Mutex::new(Some(s))))
    }

    /// The lock, if it can be had within 2 s: a compaction that holds it across its folder
    /// I/O fails the test instead of hanging it.
    fn guard(&self) -> MutexGuard<'_, Option<Store>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.0.try_lock() {
                Ok(g) => return g,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("the store lock is held across compaction's folder I/O: {e}"),
            }
        }
    }

    fn call<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(self.guard().as_mut().expect("open"))
    }
}

type Ticked = Result<Option<TickReport>, StoreError>;

/// A polling tick of `s` with `opts`, on a thread of its own.
fn spawn_tick_with(e: &Env, s: &Shared, opts: TickOptions) -> JoinHandle<Ticked> {
    s.call(|st| st.poll_now_for_test());
    let (now, mut access) = (e.clock.now(), s.clone());
    thread::spawn(move || Store::tick_phased(&mut access, now, opts))
}

fn spawn_tick(e: &Env, s: &Shared) -> JoinHandle<Ticked> {
    spawn_tick_with(e, s, TickOptions::default())
}

fn compacted(t: JoinHandle<Ticked>) -> CompactReport {
    t.join().unwrap().unwrap().expect("the tick ran").compact
}

fn originals(dir: &Path) -> usize {
    segments(dir).iter().filter(|s| !s.2.header.pack).count()
}

fn packs(dir: &Path) -> usize {
    segments(dir).iter().filter(|s| s.2.header.pack).count()
}

fn manifests(dir: &Path) -> usize {
    fs::read_dir(dir.join(".finisher").join("compaction")).map_or(0, |rd| rd.count())
}

fn assert_verifies(s: &Shared) {
    let r = s.call(|s| s.verify());
    assert!(
        r.ok,
        "{:#?}",
        r.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>()
    );
}

/// A holder (the real lease) with `n` one-record segments and a manifest past its grace.
fn manifest_past_grace(e: &Env, n: usize) -> Shared {
    let mut a = e.create("a");
    a.set_compaction_grace_for_test(GRACE);
    for i in 0..n {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        assert_eq!(e.flush(&mut a), 1);
    }
    let r = a.compact(true).unwrap();
    assert_eq!((r.manifests.len(), r.replaced), (1, n as u32), "{r:?}");
    e.advance(GRACE.as_secs() + 1);
    Shared::new(a)
}

/// A fresh reader (another server) has every record.
fn assert_fresh_reader_has(e: &Env, head: u64) {
    let f = e.open("f");
    assert_eq!(f.head(MAIN), head);
    assert!(f.verify().ok);
}

#[test]
fn short_calls_answer_while_phase_two_reads_the_folder() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactRead);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    let (head, status) = a.call(|s| (s.head(MAIN), s.status()));
    assert_eq!(head, 3);
    assert_eq!(status.compaction.pending.len(), 1);
    assert_eq!(a.call(|s| commit(s, "t3", vec![insert(0, "a1")])).rev, 4);
    hooks.release();
    let r = compacted(tick);
    assert_eq!((r.finished.len(), r.deleted), (1, 3), "{r:?}");
    assert_eq!(
        (originals(&e.dir), packs(&e.dir), manifests(&e.dir)),
        (0, 1, 0)
    );
    assert_eq!(a.call(|s| e.flush(s)), 1);
    assert_verifies(&a);
    assert_fresh_reader_has(&e, 4);
}

#[test]
fn a_commit_while_deletes_are_held_is_kept() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactDelete);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // Checked already, nothing deleted yet: a commit adds only an unexported row, which no
    // delete depends on; the deletes go on.
    assert_eq!(originals(&e.dir), 3);
    assert_eq!(a.call(|s| commit(s, "t3", vec![insert(0, "a1")])).rev, 4);
    hooks.release();
    let r = compacted(tick);
    assert_eq!((r.finished.len(), r.deleted), (1, 3), "{r:?}");
    assert_eq!(a.call(|s| e.flush(s)), 1);
    assert_verifies(&a);
    assert_fresh_reader_has(&e, 4);
}

#[test]
fn a_rebuild_during_the_unlocked_read_deletes_nothing() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactRead);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // A long call replaces the database the read is to be checked against.
    a.call(|s| s.rebuild()).unwrap();
    hooks.release();
    let r = compacted(tick);
    assert_eq!(r.deleted, 0, "the overtaken step deletes nothing: {r:?}");
    assert!(r.abandoned.is_empty(), "nor abandons: {r:?}");
    assert_eq!(originals(&e.dir), 3);
    // Later steps check afresh and finish it.
    let mut deleted = 0;
    for _ in 0..3 {
        e.advance(GRACE.as_secs() + 1);
        deleted += a.call(|s| e.poll(s)).compact.deleted;
    }
    assert_eq!(deleted, 3);
    assert_eq!(originals(&e.dir), 0);
    assert_verifies(&a);
    assert_fresh_reader_has(&e, 3);
}

#[test]
fn an_explicit_compaction_during_the_unlocked_read_overtakes_it() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactRead);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // `fin project compact` finishes the manifest meanwhile.
    let r = a.call(|s| s.compact(false)).unwrap();
    assert_eq!((r.finished.len(), r.deleted), (1, 3), "{r:?}");
    hooks.release();
    let r = compacted(tick);
    assert_eq!(r.deleted, 0, "{r:?}");
    assert!(r.abandoned.is_empty() && r.finished.is_empty(), "{r:?}");
    assert_eq!(a.call(|s| s.compaction_status().pending.len()), 0);
    assert_eq!(
        (originals(&e.dir), packs(&e.dir), manifests(&e.dir)),
        (0, 1, 0)
    );
    assert_verifies(&a);
    assert_fresh_reader_has(&e, 3);
}

#[test]
fn a_lease_change_while_deletes_are_held_stops_them() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactDelete);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // The lease in force changes (as a takeover or a lost lease would make it): the deletes
    // checked under the old one must not run.
    a.call(|s| s.set_test_lease(1, false));
    hooks.release();
    let r = compacted(tick);
    assert_eq!(r.deleted, 0, "{r:?}");
    assert_eq!((originals(&e.dir), manifests(&e.dir)), (3, 1));
}

#[test]
fn a_panic_in_the_unlocked_compaction_io_unwinds_outside_the_lock() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.panic_at(TickPoint::CompactRead);
    let tick = spawn_tick(&e, &a);
    assert!(tick.join().is_err(), "the panic reaches the tick's caller");
    // Outside the lock: not poisoned, and nothing was deleted.
    assert_eq!(a.call(|s| s.head(MAIN)), 3);
    assert_eq!(originals(&e.dir), 3);
}

#[test]
fn a_light_tick_lists_manifests_and_a_due_step_reads_without_the_lock() {
    let e = env();
    let a = manifest_past_grace(&e, 3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactList);
    let tick = spawn_tick_with(&e, &a, LIGHT);
    hooks.wait_hung();
    assert_eq!(a.call(|s| s.head(MAIN)), 3);
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!(r.compact.deleted, 0, "the light tick deletes nothing");
    assert!(a.call(|s| s.due_work().compaction));
    // The due step, as fin-node runs it after the tick.
    hooks.hang(TickPoint::CompactRead);
    let mut access = a.clone();
    let step = thread::spawn(move || Store::run_due_compaction_step_phased(&mut access));
    hooks.wait_hung();
    assert_eq!(a.call(|s| commit(s, "t3", vec![insert(0, "a1")])).rev, 4);
    hooks.release();
    let r = step.join().unwrap().unwrap().expect("the step ran");
    assert_eq!((r.finished.len(), r.deleted), (1, 3), "{r:?}");
    assert!(!a.call(|s| s.due_work().compaction));
    assert_eq!(a.call(|s| e.flush(s)), 1);
    assert_verifies(&a);
    assert_fresh_reader_has(&e, 4);
}

#[test]
fn phase_one_writes_its_packs_without_the_lock() {
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(1, true);
    a.set_compaction_grace_for_test(GRACE);
    let n = fin_store::COMPACT_TRIGGER + 1;
    for i in 0..n {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        // Fixed lease: the clock may jump (the export delay grows under churn).
        a.poll_now_for_test();
        a.tick_with(e.clock.now(), LIGHT).unwrap();
        e.clock.advance(Duration::from_secs(31));
        a.poll_now_for_test();
        a.tick_with(e.clock.now(), LIGHT).unwrap();
    }
    assert_eq!((originals(&e.dir), manifests(&e.dir)), (n, 0));
    let a = Shared::new(a);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::CompactWrite);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // Planned from the rows exported; a commit meanwhile is not in the pack.
    let rev = a.call(|s| commit(s, "late", vec![insert(0, "a1")])).rev;
    assert_eq!(rev, n as u64 + 1);
    hooks.release();
    let r = compacted(tick);
    assert_eq!((r.manifests.len(), r.packs_written), (1, 1), "{r:?}");
    assert_eq!(r.replaced as usize, n);
    assert_eq!(manifests(&e.dir), 1);
    e.clock.advance(GRACE + Duration::from_secs(1));
    let r = a.call(|s| e.poll(s)).compact;
    assert_eq!(r.deleted as usize, n, "{r:?}");
    assert_eq!(packs(&e.dir), 1);
    assert_verifies(&a);
}
