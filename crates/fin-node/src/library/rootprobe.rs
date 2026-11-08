//! Root probes (plan 5D Decision 3): a root's state off the JS thread and off libuv's pool.
//!
//! The measurement is `fin_library::root_state`, which is **single-flight per root**: it runs
//! `stat` on a thread of its own, a call while one is pending waits on that one (up to its own
//! timeout) instead of starting another, and a call past its deadline answers `Timeout` and
//! leaves the thread to finish (a dead hard mount blocks that thread only). Each call here
//! waits on a short thread of its own, so the caller never blocks.
//!
//! What this module adds is the count of **stuck** roots: those whose latest probe timed out
//! (`libraryProbeStatus().stuckProbes`). A root leaves the set when a later probe of it
//! answers anything else.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use fin_library::{RootState, VolumeIdentity, root_state};

fn stuck() -> MutexGuard<'static, HashSet<String>> {
    static STUCK: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    STUCK
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Roots whose latest probe timed out.
pub fn stuck_roots() -> u32 {
    u32::try_from(stuck().len()).unwrap_or(u32::MAX)
}

/// The state of `root` against `stored`, given to `reply` on a thread of its own.
pub fn probe(
    root: String,
    stored: VolumeIdentity,
    timeout: Duration,
    reply: impl FnOnce(RootState) + Send + 'static,
) {
    let reply = std::sync::Arc::new(Mutex::new(Some(reply)));
    let from_thread = std::sync::Arc::clone(&reply);
    let key = root.clone();
    let spawned = std::thread::Builder::new()
        .name("fin-node-rootprobe".into())
        .spawn(move || {
            let state = root_state(Path::new(&root), &stored, timeout);
            if matches!(state, RootState::Timeout) {
                stuck().insert(key);
            } else {
                stuck().remove(&key);
            }
            if let Some(reply) = take(&from_thread) {
                reply(state);
            }
        });
    // A failed spawn drops the closure, not `reply`: the promise still settles.
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

/// Test builds: make a root's stat block, as a dead hard mount does (`fin-library`'s hook).
#[cfg(feature = "test-hooks")]
pub mod hooks {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    use fin_library::volume::probe_hooks;

    fn held() -> &'static Mutex<HashMap<String, probe_hooks::Block>> {
        static HELD: OnceLock<Mutex<HashMap<String, probe_hooks::Block>>> = OnceLock::new();
        HELD.get_or_init(Mutex::default)
    }

    pub fn set_hung(root: &str, on: bool) {
        let mut g = held().lock().unwrap_or_else(|p| p.into_inner());
        if on {
            g.entry(root.to_owned())
                .or_insert_with(|| probe_hooks::block(Path::new(root)));
        } else {
            // Dropping the guard releases the blocked probes.
            g.remove(root);
        }
    }

    pub fn started(root: &str) -> u32 {
        u32::try_from(probe_hooks::started(Path::new(root))).unwrap_or(u32::MAX)
    }
}
