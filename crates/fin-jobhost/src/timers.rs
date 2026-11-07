//! One timer thread over a binary heap (monotonic `Instant`). Callbacks run on it and must be
//! quick: they check state and post commands. A check that is not due yet re-arms itself.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

type Task = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct State {
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    tasks: HashMap<u64, Task>,
    next: u64,
    stop: bool,
}

pub(crate) struct Timers {
    shared: Arc<(Mutex<State>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Timers {
    pub fn start() -> std::io::Result<Timers> {
        let shared: Arc<(Mutex<State>, Condvar)> = Arc::default();
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("jobhost-timer".into())
            .spawn(move || run(&s))?;
        Ok(Timers {
            shared,
            thread: Some(thread),
        })
    }

    /// Runs `f` at `when` (at once if it has passed).
    pub fn at(&self, when: Instant, f: impl FnOnce() + Send + 'static) -> u64 {
        let (lock, cv) = &*self.shared;
        let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
        st.next += 1;
        let id = st.next;
        st.heap.push(Reverse((when, id)));
        st.tasks.insert(id, Box::new(f));
        cv.notify_one();
        id
    }
}

fn run(shared: &(Mutex<State>, Condvar)) {
    let (lock, cv) = shared;
    let mut st = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if st.stop {
            return;
        }
        let now = Instant::now();
        match st.heap.peek().copied() {
            Some(Reverse((when, id))) if when <= now => {
                st.heap.pop();
                if let Some(task) = st.tasks.remove(&id) {
                    drop(st);
                    task();
                    st = lock.lock().unwrap_or_else(|e| e.into_inner());
                }
            }
            Some(Reverse((when, _))) => {
                st = cv
                    .wait_timeout(st, when - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            None => st = cv.wait(st).unwrap_or_else(|e| e.into_inner()),
        }
    }
}

impl Drop for Timers {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        cv.notify_one();
        if let Some(t) = self.thread.take() {
            // A callback may hold the last reference to the host and drop it here.
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn tasks_run_in_deadline_order() {
        let t = Timers::start().unwrap();
        let (tx, rx) = mpsc::channel();
        let now = Instant::now();
        for (i, ms) in [(1, 60u64), (2, 20), (3, 40)] {
            let tx = tx.clone();
            t.at(now + Duration::from_millis(ms), move || tx.send(i).unwrap());
        }
        let got: Vec<i32> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        assert_eq!(got, [2, 3, 1]);
        assert!(now.elapsed() >= Duration::from_millis(60));
    }
}
