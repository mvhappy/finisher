//! The phased tick (`Store::tick_phased`): folder I/O runs outside the store lock, so short
//! calls do not wait for a follower's import or a holder's export, and what changed meanwhile
//! is checked again before anything is applied.

mod integration_support;

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fin_store::lease::MonoClock;
use fin_store::{
    MAIN, ReadOnlyReason, Store, StoreAccess, StoreError, StoreEvent, TickOptions, TickPoint,
    TickReport,
};
use integration_support::*;

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

    /// The lock, if it can be had within 2 s: a tick that holds it across its folder I/O
    /// fails the test instead of hanging it.
    fn guard(&self) -> MutexGuard<'_, Option<Store>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.0.try_lock() {
                Ok(g) => return g,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("the store lock is held across the tick's folder I/O: {e}"),
            }
        }
    }

    /// Runs `f` on the store (a short call).
    fn call<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(self.guard().as_mut().expect("open"))
    }
}

type Ticked = Result<Option<TickReport>, StoreError>;

/// Starts a polling tick of `s` on a thread of its own.
fn spawn_tick(e: &Env, s: &Shared) -> JoinHandle<Ticked> {
    s.call(|st| st.poll_now_for_test());
    let (now, mut access) = (e.clock.now(), s.clone());
    thread::spawn(move || Store::tick_phased(&mut access, now, TickOptions::default()))
}

fn applied(r: &TickReport) -> usize {
    r.events
        .iter()
        .filter(|e| matches!(e, StoreEvent::Applied(_)))
        .count()
}

fn rewound(r: &TickReport) -> bool {
    r.events
        .iter()
        .any(|e| matches!(e, StoreEvent::Rewound { .. }))
}

/// A writer `a` (holding, `t1` exported) and a follower `b` that has `t1`.
fn writer_and_follower(e: &Env) -> (Store, Shared) {
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let b = Shared::new(e.open("b"));
    assert_eq!(b.call(|s| s.head(MAIN)), 1);
    (a, b)
}

#[test]
fn a_follower_answers_short_calls_while_its_tick_imports() {
    let e = env();
    let (mut a, b) = writer_and_follower(&e);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    e.flush(&mut a);
    let hooks = b.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ImportRead);
    let tick = spawn_tick(&e, &b);
    hooks.wait_hung();
    // Between the scan and the apply: reads answer, from the state before the pass.
    let (head, status) = b.call(|s| (s.head(MAIN), s.status()));
    assert_eq!(head, 1);
    assert_eq!(status.project_id, a.project_id());
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!(r.imports.len(), 1);
    assert_eq!(applied(&r), 2);
    assert_eq!(b.call(|s| s.head(MAIN)), 3);
}

#[test]
fn a_commit_during_an_unlocked_export_goes_into_the_next_segment() {
    let e = env();
    let a = Shared::new(e.create("a"));
    a.call(|s| commit(s, "t1", vec![asset("/m/a.mov")]));
    a.call(|s| e.poll(s)); // starts the export delay
    e.advance(3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ExportWrite);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    let c = a.call(|s| commit(s, "t2", vec![insert(0, "a1")]));
    assert_eq!(c.rev, 2);
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!((r.export.segments_written, r.export.records), (1, 1));
    assert_eq!(r.export.error, None);
    assert_eq!(folder_tx_ids(&e.dir), ["t1"]);
    // The commit made meanwhile is exported next, chained to the segment written.
    assert_eq!(a.call(|s| e.flush(s)), 1);
    assert_eq!(folder_tx_ids(&e.dir), ["t1", "t2"]);
    let segs = segments(&e.dir);
    assert_eq!(
        segs[1].2.header.prev_segment_hash.as_deref(),
        Some(segs[0].2.hash.as_str())
    );
    let f = e.open("f");
    assert_eq!(f.head(MAIN), 2);
    assert_eq!(f.read_only(), Some(ReadOnlyReason::LeaseHeld));
}

#[test]
fn closing_during_an_unlocked_import_ends_the_tick() {
    let e = env();
    let (mut a, b) = writer_and_follower(&e);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    let hooks = b.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ImportRead);
    let tick = spawn_tick(&e, &b);
    hooks.wait_hung();
    let store = b.guard().take().expect("open");
    store.close().unwrap();
    hooks.release();
    assert!(
        tick.join().unwrap().unwrap().is_none(),
        "the store is gone: nothing more to do"
    );
    // The next open imports what the abandoned pass had read.
    assert_eq!(e.open("b").head(MAIN), 2);
}

#[test]
fn only_one_tick_runs_at_a_time() {
    let e = env();
    let (mut a, b) = writer_and_follower(&e);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    let hooks = b.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ImportRead);
    let first = spawn_tick(&e, &b);
    hooks.wait_hung();
    let (tx, rx) = mpsc::channel();
    let (now, mut access) = (e.clock.now(), b.clone());
    thread::spawn(move || {
        let _ = tx.send(Store::tick_phased(&mut access, now, TickOptions::default()));
    });
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("the second tick returns at once");
    assert!(second.unwrap().is_none(), "skipped: a tick is running");
    hooks.release();
    let r = first.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!(applied(&r), 1);
}

#[test]
fn an_import_applied_meanwhile_discards_the_paused_pass() {
    let e = env();
    let (mut a, b) = writer_and_follower(&e);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    let hooks = b.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ImportRead);
    let tick = spawn_tick(&e, &b);
    hooks.wait_hung();
    // The paused pass saw t1..t2; t3 arrives and another pass (a takeover's, a rebuild's)
    // applies it before the paused one gets the lock.
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    e.flush(&mut a);
    let r = b.call(|s| s.import()).unwrap();
    assert_eq!(r.applied.len(), 2);
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert!(
        r.imports.is_empty(),
        "the paused pass is not applied: {r:?}"
    );
    assert!(!rewound(&r), "a stale folder view must not rewind: {r:?}");
    assert_eq!(applied(&r), 0);
    assert_eq!(b.call(|s| s.head(MAIN)), 3);
    assert_eq!(b.call(|s| s.read_only()), Some(ReadOnlyReason::LeaseHeld));
    let r = b.call(|s| e.poll(s));
    assert!(!rewound(&r) && applied(&r) == 0, "{r:?}");
    assert_eq!(b.call(|s| s.head(MAIN)), 3);
}

#[test]
fn a_rebuild_during_an_unlocked_export_leaves_no_error() {
    let e = env();
    let a = Shared::new(e.create("a"));
    a.call(|s| commit(s, "t1", vec![asset("/m/a.mov")]));
    a.call(|s| e.poll(s));
    e.advance(3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ExportWrite);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // A long call flushes the same records and replaces the database meanwhile.
    a.call(|s| s.rebuild()).unwrap();
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!(r.export.error, None);
    assert_eq!(r.export.segments_written, 0, "{r:?}");
    assert_eq!(folder_tx_ids(&e.dir), ["t1"]);
    assert_eq!(a.call(|s| s.export_error().cloned()), None);
    let r = a.call(|s| e.poll(s));
    assert_eq!(r.export.segments_written, 0);
    assert_eq!(a.call(|s| s.read_only()), None);
}

#[test]
fn a_lease_let_go_during_an_unlocked_export_writes_nothing() {
    let e = env();
    let a = Shared::new(e.create("a"));
    a.call(|s| commit(s, "t1", vec![asset("/m/a.mov")]));
    a.call(|s| e.poll(s));
    e.advance(3);
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ExportWrite);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // The store lets its lease go (as a newer project would make it): the record in the
    // folder is still ours and fresh, but the paused write must not use the lease any more.
    a.call(|s| s.set_test_lease(1, false));
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert_eq!((r.export.segments_written, r.export.records), (0, 0));
    assert!(folder_tx_ids(&e.dir).is_empty());
}

#[test]
fn a_project_file_read_overtaken_by_set_collect_is_not_installed() {
    let e = env();
    let a = Shared::new(e.create("a"));
    assert_eq!(
        a.call(|s| s.collect_policy()),
        fin_store::CollectPolicy::Default
    );
    let hooks = a.call(|s| s.tick_hooks());
    hooks.hang(TickPoint::ImportRead);
    let tick = spawn_tick(&e, &a);
    hooks.wait_hung();
    // The paused pass read the project file before this write.
    a.call(|s| s.set_collect(fin_store::CollectPolicy::Never))
        .unwrap();
    hooks.release();
    let r = tick.join().unwrap().unwrap().expect("the tick ran");
    assert!(
        r.imports.is_empty(),
        "the overtaken pass is not applied: {r:?}"
    );
    assert_eq!(
        a.call(|s| s.collect_policy()),
        fin_store::CollectPolicy::Never
    );
    a.call(|s| e.poll(s));
    assert_eq!(
        a.call(|s| s.collect_policy()),
        fin_store::CollectPolicy::Never
    );
}
