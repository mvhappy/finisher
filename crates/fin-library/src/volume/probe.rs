//! [`super::root_state`]'s measurement thread: **one in flight per root**. A probe of a dead
//! hard mount blocks in the kernel until the mount answers; re-probing it on every status poll
//! would leave one stuck thread per call. A caller that finds a probe pending waits on that
//! one (up to its own timeout) instead of starting another, so a dead mount costs one thread
//! however often it is asked about.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::source::RootStat;

/// A probe's answer: the facts, or [`super::unreachable_code`] of its error.
pub(crate) type Answer = Result<RootStat, &'static str>;

#[derive(Default)]
struct Probe {
    answer: Mutex<Option<Answer>>,
    ready: Condvar,
}

static PENDING: Mutex<Option<HashMap<PathBuf, Arc<Probe>>>> = Mutex::new(None);

fn pending() -> MutexGuard<'static, Option<HashMap<PathBuf, Arc<Probe>>>> {
    PENDING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The answer of the probe of `root`, joining the pending one or starting it; `None` past
/// `timeout`; `Err("THREAD")` when no thread could start.
pub(crate) fn run(root: &Path, timeout: Duration) -> Option<Answer> {
    let probe = {
        let mut g = pending();
        let map = g.get_or_insert_with(HashMap::new);
        match map.get(root) {
            Some(p) => Arc::clone(p),
            None => {
                let p = Arc::new(Probe::default());
                let mine = Arc::clone(&p);
                let path = root.to_path_buf();
                let spawned = std::thread::Builder::new()
                    .name("fin-library-root-stat".into())
                    .spawn(move || measure(&path, &mine));
                if spawned.is_err() {
                    return Some(Err("THREAD"));
                }
                map.insert(root.to_path_buf(), Arc::clone(&p));
                p
            }
        }
    };
    let until = Instant::now() + timeout;
    let mut answer = probe.answer.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if let Some(a) = answer.as_ref() {
            return Some(a.clone());
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        answer = probe
            .ready
            .wait_timeout(answer, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

fn measure(path: &Path, probe: &Arc<Probe>) {
    hooks::on_probe(path);
    let answer = super::root_stat(path).map_err(|e| super::unreachable_code(&e));
    // The probe leaves the pending map before its answer is published: a caller that comes
    // after the answer exists starts a fresh measurement instead of joining a finished one (it
    // would otherwise be told what the root was, not what it is).
    {
        let mut g = pending();
        if let Some(map) = g.as_mut()
            && map.get(path).is_some_and(|p| Arc::ptr_eq(p, probe))
        {
            map.remove(path);
        }
    }
    *probe.answer.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
    probe.ready.notify_all();
}

/// Test hooks (feature `test-hooks`): hold a root's probe before it measures, and count the
/// probes started per root. Without the feature they are inert.
#[cfg(feature = "test-hooks")]
pub mod hooks {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Condvar, Mutex, PoisonError};

    type Gate = Arc<(Mutex<bool>, Condvar)>;

    static GATES: Mutex<Option<HashMap<PathBuf, Gate>>> = Mutex::new(None);
    static STARTED: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

    /// While held, probes of `root` block before measuring (a dead mount); dropping it
    /// releases them.
    pub struct Block {
        root: PathBuf,
        gate: Gate,
    }

    impl Drop for Block {
        fn drop(&mut self) {
            let mut g = GATES.lock().unwrap_or_else(PoisonError::into_inner);
            g.get_or_insert_with(HashMap::new).remove(&self.root);
            *self.gate.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
            self.gate.1.notify_all();
        }
    }

    pub fn block(root: &Path) -> Block {
        let gate: Gate = Arc::default();
        GATES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(root.to_path_buf(), Arc::clone(&gate));
        Block {
            root: root.to_path_buf(),
            gate,
        }
    }

    /// Probes of `root` started so far.
    pub fn started(root: &Path) -> u64 {
        let g = STARTED.lock().unwrap_or_else(PoisonError::into_inner);
        g.as_ref().and_then(|m| m.get(root).copied()).unwrap_or(0)
    }

    pub(crate) fn on_probe(root: &Path) {
        *STARTED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .entry(root.to_path_buf())
            .or_default() += 1;
        let gate = GATES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|m| m.get(root).cloned());
        if let Some(gate) = gate {
            let mut open = gate.0.lock().unwrap_or_else(PoisonError::into_inner);
            while !*open {
                open = gate.1.wait(open).unwrap_or_else(PoisonError::into_inner);
            }
        }
    }
}

#[cfg(not(feature = "test-hooks"))]
mod hooks {
    pub(crate) fn on_probe(_: &std::path::Path) {}
}
