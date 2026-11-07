//! Root probes (plan 5D Decision 3): one outstanding measurement per root, on its own OS
//! thread, never on libuv's pool, so a `stat` that hangs on a dead NFS hard mount blocks that
//! thread and nothing else.
//!
//! - A probe of a root with none outstanding starts one **stat thread** (`root_stat`, which
//!   opens no file) and one short **waiter thread** that gives up at the caller's deadline.
//! - A probe of a root whose stat thread is still running **joins it**: no second stat is
//!   issued, each caller classifies the one measurement against its own stored identity.
//! - A waiter that reaches its deadline answers `timeout` and marks the stat thread **stuck**
//!   (`stuckProbes` in [`status`]); the thread is left to finish. While it is stuck every
//!   probe of that root answers `timeout` at once, without a new thread. The stat thread
//!   clears the mark when it returns (the mount came back or failed).
//!
//! The measurement and the judgement are `fin-library`'s (`root_stat`, `classify_root`).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use fin_library::volume::{root_stat, unreachable_code};
use fin_library::{RootStat, RootState, VolumeIdentity, classify_root};

/// What a stat thread measured: the root's facts, or the short code of why it is unreachable.
type Measured = Arc<Result<RootStat, String>>;

struct Slot {
    /// Which stat thread this slot belongs to (a waiter never marks a later run).
    run: u64,
    stuck: bool,
    waiters: Vec<mpsc::Sender<Measured>>,
}

#[derive(Default)]
struct Roots {
    next_run: u64,
    slots: HashMap<String, Slot>,
}

fn roots() -> MutexGuard<'static, Roots> {
    static ROOTS: OnceLock<Mutex<Roots>> = OnceLock::new();
    ROOTS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

static STUCK: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicU32 = AtomicU32::new(0);

/// What the probes are doing now (`library.status` in Task 10 reports `stuckProbes`).
pub struct Status {
    /// Stat threads past a caller's deadline and not yet returned.
    pub stuck: u32,
    /// Roots with a stat thread running.
    pub outstanding: u32,
    /// Stat threads started since the process began.
    pub started: u32,
}

pub fn status() -> Status {
    Status {
        stuck: STUCK.load(Ordering::SeqCst),
        outstanding: u32::try_from(roots().slots.len()).unwrap_or(u32::MAX),
        started: STARTED.load(Ordering::SeqCst),
    }
}

/// The state of `root` against `stored`, given to `reply` (on a waiter thread, or at once when
/// the root's stat thread is stuck). Never blocks the caller.
pub fn probe(
    root: String,
    stored: VolumeIdentity,
    timeout: Duration,
    reply: impl FnOnce(RootState) + Send + 'static,
) {
    let (tx, rx) = mpsc::channel::<Measured>();
    let run = {
        let mut g = roots();
        match g.slots.get_mut(&root) {
            Some(slot) if slot.stuck => {
                drop(g);
                return reply(RootState::Timeout);
            }
            Some(slot) => {
                slot.waiters.push(tx);
                slot.run
            }
            None => {
                g.next_run += 1;
                let run = g.next_run;
                g.slots.insert(
                    root.clone(),
                    Slot {
                        run,
                        stuck: false,
                        waiters: vec![tx],
                    },
                );
                if let Err(code) = spawn_stat(&root, run) {
                    g.slots.remove(&root);
                    drop(g);
                    return reply(RootState::Unreachable { code });
                }
                run
            }
        }
    };
    let key = root.clone();
    // Shared with the spawn's failure path, so the promise always settles.
    let reply = Arc::new(Mutex::new(Some(reply)));
    let spawn_reply = Arc::clone(&reply);
    let spawned = std::thread::Builder::new()
        .name("fin-node-rootprobe-wait".into())
        .spawn(move || {
            let state = match rx.recv_timeout(timeout) {
                Ok(m) => state_of(&stored, &m),
                Err(RecvTimeoutError::Timeout) => {
                    mark_stuck(&key, run);
                    RootState::Timeout
                }
                Err(RecvTimeoutError::Disconnected) => RootState::Unreachable {
                    code: "THREAD".into(),
                },
            };
            if let Some(reply) = take(&spawn_reply) {
                reply(state);
            }
        });
    if spawned.is_err()
        && let Some(reply) = take(&reply)
    {
        reply(RootState::Unreachable {
            code: "THREAD".into(),
        });
    }
}

fn take<F>(r: &Mutex<Option<F>>) -> Option<F> {
    r.lock().unwrap_or_else(|p| p.into_inner()).take()
}

fn state_of(stored: &VolumeIdentity, m: &Measured) -> RootState {
    match &**m {
        Ok(now) => classify_root(stored, now),
        Err(code) => RootState::Unreachable { code: code.clone() },
    }
}

fn mark_stuck(root: &str, run: u64) {
    let mut g = roots();
    if let Some(slot) = g.slots.get_mut(root)
        && slot.run == run
        && !slot.stuck
    {
        slot.stuck = true;
        STUCK.fetch_add(1, Ordering::SeqCst);
    }
}

fn spawn_stat(root: &str, run: u64) -> Result<(), String> {
    let key = root.to_owned();
    let path = PathBuf::from(root);
    STARTED.fetch_add(1, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("fin-node-rootprobe".into())
        .spawn(move || {
            #[cfg(feature = "test-hooks")]
            hooks::wait_while_hung(&key);
            let measured: Measured = Arc::new(
                catch_unwind(AssertUnwindSafe(|| root_stat(&path)))
                    .unwrap_or_else(|_| Err(std::io::Error::other("the root measurement panicked")))
                    .map_err(|e| unreachable_code(&e).to_owned()),
            );
            finish(&key, run, &measured);
        })
        .map(drop)
        .map_err(|_| "THREAD".to_owned())
}

/// Frees the root's slot and answers every waiter that is still there.
fn finish(root: &str, run: u64, measured: &Measured) {
    let waiters = {
        let mut g = roots();
        match g.slots.get(root) {
            Some(slot) if slot.run == run => {}
            _ => return,
        }
        let slot = g.slots.remove(root).expect("checked above");
        if slot.stuck {
            STUCK.fetch_sub(1, Ordering::SeqCst);
        }
        slot.waiters
    };
    for w in waiters {
        // A waiter that timed out is gone; its channel is closed.
        let _ = w.send(Arc::clone(measured));
    }
}

/// Test builds: make a root's stat block, as a dead hard mount does.
#[cfg(feature = "test-hooks")]
pub mod hooks {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    fn hung() -> &'static Mutex<HashSet<String>> {
        static HUNG: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        HUNG.get_or_init(Mutex::default)
    }

    pub fn set_hung(root: &str, on: bool) {
        let mut g = hung().lock().unwrap_or_else(|p| p.into_inner());
        if on {
            g.insert(root.to_owned());
        } else {
            g.remove(root);
        }
    }

    pub(super) fn wait_while_hung(root: &str) {
        while hung()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(root)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
