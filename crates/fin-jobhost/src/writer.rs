//! A process's stdin: a bounded queue (256 messages or 4 MiB, Decision 11) drained by its own
//! thread. Enqueueing never blocks; a full queue means the process is not reading.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};

use fin_jobs::HostMsg;
use fin_jobs::codec;

use crate::SendError;

pub(crate) const MAX_MESSAGES: usize = 256;
pub(crate) const MAX_BYTES: usize = 4 << 20;

#[derive(Default)]
struct Queue {
    lines: VecDeque<Vec<u8>>,
    bytes: usize,
    seq: u64,
    /// `close_stdin`: the pipe closes once the queue is written.
    closing: bool,
    /// The pipe is gone (the process exited or closed its stdin).
    dead: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Writer {
    q: Arc<(Mutex<Queue>, Condvar)>,
}

impl Writer {
    /// Frames `msg` with the next `seq` and enqueues it.
    pub fn send(&self, msg: &HostMsg) -> Result<(), SendError> {
        let (lock, cv) = &*self.q;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if q.dead || q.closing {
            return Err(SendError::Gone);
        }
        let line = codec::encode(q.seq + 1, msg).map_err(|e| SendError::Encode(e.to_string()))?;
        if q.lines.len() >= MAX_MESSAGES || q.bytes + line.len() > MAX_BYTES {
            return Err(SendError::QueueFull);
        }
        q.seq += 1;
        q.bytes += line.len();
        q.lines.push_back(line);
        cv.notify_one();
        Ok(())
    }

    /// Closes the pipe once what is queued has been written.
    pub fn close(&self) {
        let (lock, cv) = &*self.q;
        lock.lock().unwrap_or_else(|e| e.into_inner()).closing = true;
        cv.notify_one();
    }

    /// The process is gone: drop the queue and stop.
    pub fn kill(&self) {
        let (lock, cv) = &*self.q;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.dead = true;
        q.lines.clear();
        q.bytes = 0;
        cv.notify_one();
    }

    /// The writer thread's loop: writes lines until closed, killed or the pipe breaks.
    pub fn run(&self, mut pipe: impl Write) {
        let (lock, cv) = &*self.q;
        loop {
            let line = {
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if q.dead {
                        return;
                    }
                    if let Some(line) = q.lines.pop_front() {
                        q.bytes -= line.len();
                        break line;
                    }
                    if q.closing {
                        return;
                    }
                    q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            // `write_all` retries `Interrupted`.
            if pipe.write_all(&line).and_then(|()| pipe.flush()).is_err() {
                self.kill();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_bounded_by_count_and_bytes() {
        let w = Writer::default();
        for _ in 0..MAX_MESSAGES {
            w.send(&HostMsg::Shutdown {}).unwrap();
        }
        assert_eq!(w.send(&HostMsg::Shutdown {}), Err(SendError::QueueFull));
        let w = Writer::default();
        let big = HostMsg::Reply {
            id: 1,
            value: Some(serde_json::Value::String("x".repeat(1 << 20))),
            error: None,
        };
        assert!(matches!(w.send(&big), Err(SendError::Encode(_))));
        let mid = HostMsg::Reply {
            id: 1,
            value: Some(serde_json::Value::String("x".repeat(900 << 10))),
            error: None,
        };
        for _ in 0..4 {
            w.send(&mid).unwrap();
        }
        assert_eq!(w.send(&mid), Err(SendError::QueueFull));
    }

    #[test]
    fn lines_are_written_in_order_with_seq_and_close_ends() {
        let w = Writer::default();
        w.send(&HostMsg::Shutdown {}).unwrap();
        w.send(&HostMsg::Shutdown {}).unwrap();
        w.close();
        assert_eq!(w.send(&HostMsg::Shutdown {}), Err(SendError::Gone));
        let mut out = Vec::new();
        w.run(&mut out);
        let text = String::from_utf8(out).unwrap();
        let seqs: Vec<u64> = text
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, [1, 2]);
    }
}
