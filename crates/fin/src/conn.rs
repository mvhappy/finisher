//! A blocking line connection with a per-operation timeout on every platform.
//!
//! Unix sockets support `SO_RCVTIMEO`/`SO_SNDTIMEO`, but synchronous Windows named pipes have
//! no I/O timeouts (`interprocess` returns `Unsupported`). So the stream lives on a worker
//! thread that runs one operation at a time (a synchronous pipe handle serializes reads and
//! writes anyway), and the caller waits for each result with a deadline. After a timeout the
//! worker may still be blocked in the OS call, so the connection is poisoned: every later
//! operation fails with `TimedOut` too. A stuck worker ends with the process (fin is one-shot).
//!
//! Lines are bounded ([`MAX_LINE`]): a server line longer than that fails with
//! [`FrameTooLarge`] instead of growing fin's memory without limit (5A Decision 36).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

/// The longest line fin reads from the server: 16 MiB, the frame limit spec §4.3 sets for both
/// sides (finisherd does not cap its own inbound lines yet: 5A Task 3).
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// The error payload of a line longer than [`MAX_LINE`] (`io::ErrorKind::InvalidData`).
#[derive(Debug)]
pub struct FrameTooLarge {
    pub max: usize,
}

impl std::fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a server frame is longer than {} bytes", self.max)
    }
}

impl std::error::Error for FrameTooLarge {}

/// True when `e` is a [`FrameTooLarge`].
pub fn is_frame_too_large(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<FrameTooLarge>())
}

enum Op {
    Write(Vec<u8>),
    ReadLine,
}

/// Reads up to and including the next `\n`, at most `max` bytes (else [`FrameTooLarge`]).
fn read_bounded_line(conn: &mut impl BufRead, max: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        let available = match conn.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            break;
        }
        let (take, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if bytes.len() + take > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                FrameTooLarge { max },
            ));
        }
        bytes.extend_from_slice(&available[..take]);
        conn.consume(take);
        if done {
            break;
        }
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub struct TimedConn {
    ops: Sender<Op>,
    results: Receiver<io::Result<String>>,
    timeout: Duration,
    poisoned: bool,
}

impl TimedConn {
    pub fn new<S: Read + Write + Send + 'static>(stream: S, timeout: Duration) -> TimedConn {
        let (ops, op_rx) = mpsc::channel::<Op>();
        let (result_tx, results) = mpsc::channel();
        thread::spawn(move || {
            let mut conn = BufReader::new(stream);
            for op in op_rx {
                let result = match op {
                    Op::Write(bytes) => {
                        let out = conn.get_mut();
                        out.write_all(&bytes)
                            .and_then(|()| out.flush())
                            .map(|()| String::new())
                    }
                    Op::ReadLine => read_bounded_line(&mut conn, MAX_LINE),
                };
                if result_tx.send(result).is_err() {
                    break;
                }
            }
        });
        TimedConn {
            ops,
            results,
            timeout,
            poisoned: false,
        }
    }

    /// The timeout of later operations (the overall deadline shortens it as it nears).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    fn run(&mut self, op: Op) -> io::Result<String> {
        if self.poisoned {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "an earlier operation on this connection timed out",
            ));
        }
        let stopped = || io::Error::new(io::ErrorKind::BrokenPipe, "the connection worker stopped");
        self.ops.send(op).map_err(|_| stopped())?;
        match self.results.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.poisoned = true;
                Err(io::ErrorKind::TimedOut.into())
            }
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// Writes and flushes `bytes`.
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.run(Op::Write(bytes.to_vec())).map(|_| ())
    }

    /// Reads up to and including the next `\n`; an empty string is end of stream, and a string
    /// without a trailing `\n` means the peer closed mid-line.
    pub fn read_line(&mut self) -> io::Result<String> {
        self.run(Op::ReadLine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that accepts writes and never answers.
    struct Silent(Receiver<()>);

    impl Read for Silent {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            let _ = self.0.recv(); // blocks until the test ends
            Ok(0)
        }
    }

    impl Write for Silent {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_blocked_read_times_out_and_poisons_the_connection() {
        let (_hold, rx) = mpsc::channel();
        let mut conn = TimedConn::new(Silent(rx), Duration::from_millis(100));
        conn.write_all(b"hello\n").unwrap();
        assert_eq!(
            conn.read_line().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            conn.write_all(b"again\n").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn lines_are_bounded() {
        let mut ok = io::Cursor::new(b"0123456789\nabc".to_vec());
        assert_eq!(read_bounded_line(&mut ok, 11).unwrap(), "0123456789\n");
        assert_eq!(read_bounded_line(&mut ok, 11).unwrap(), "abc");
        let mut long = BufReader::with_capacity(4, io::Cursor::new(b"0123456789\n".to_vec()));
        let e = read_bounded_line(&mut long, 10).unwrap_err();
        assert!(is_frame_too_large(&e));
        assert!(!is_frame_too_large(&io::Error::other("x")));
    }

    #[test]
    fn lines_pass_through_and_end_of_stream_is_empty() {
        let data = io::Cursor::new(b"one\ntwo".to_vec());
        struct Rw(io::Cursor<Vec<u8>>);
        impl Read for Rw {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.0.read(b)
            }
        }
        impl Write for Rw {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut conn = TimedConn::new(Rw(data), Duration::from_secs(5));
        assert_eq!(conn.read_line().unwrap(), "one\n");
        assert_eq!(conn.read_line().unwrap(), "two");
        assert_eq!(conn.read_line().unwrap(), "");
    }
}
