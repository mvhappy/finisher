//! The file the tests read: a JSON array with one record per fake process, appended in start
//! order and never removed, so a test can count the processes a resume started. A record holds
//! the flags, environment NAMES (never values), the control traffic, answers and spawns.
//!
//! Several processes may share the file: each update takes an OS lock on `<file>.lock` (a file
//! that is never removed, so a dead holder's lock ends with it and no lock is ever broken),
//! rewrites the array with its own record replaced (temp file, then rename) and unlocks. Each
//! update writes the whole record, so one that could not be written is carried by the next; a
//! failure is said on stderr, and a file that is not a JSON array is never overwritten.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

/// How long an update waits for another process's lock before it says so on stderr.
const LOCK_WAIT: Duration = Duration::from_secs(30);

pub struct Observed {
    path: Option<PathBuf>,
    id: String,
    doc: Map<String, Value>,
}

impl Observed {
    pub fn new(path: Option<PathBuf>) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Observed {
            path,
            id: format!("{}-{nanos}", std::process::id()),
            doc: Map::new(),
        }
    }

    /// Records the invocation: raw argv, flags, and the names of the environment, sorted.
    pub fn start(&mut self, argv: &[String], flags: Value) {
        let mut names: Vec<String> = std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        names.sort();
        self.doc.insert("id".into(), json!(self.id));
        self.doc.insert("pid".into(), json!(std::process::id()));
        self.doc.insert("argv".into(), json!(argv));
        self.doc.insert("flags".into(), flags);
        self.doc.insert("envNames".into(), json!(names));
        self.flush();
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.doc.insert(key.into(), value);
        self.flush();
    }

    pub fn push(&mut self, key: &str, value: Value) {
        if let Value::Array(a) = self.doc.entry(key).or_insert_with(|| json!([])) {
            a.push(value);
        }
        self.flush();
    }

    fn flush(&self) {
        let Some(p) = &self.path else { return };
        let Some(_lock) = Lock::take(p) else {
            eprintln!(
                "observed: {} stayed locked for {LOCK_WAIT:?}; this update waits for the next",
                p.display()
            );
            return;
        };
        let mut all: Vec<Value> = match std::fs::read_to_string(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => {
                eprintln!("observed: {}: {e}", p.display());
                return;
            }
            Ok(t) => match serde_json::from_str(&t) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "observed: {} is not a JSON array ({e}); left as it is",
                        p.display()
                    );
                    return;
                }
            },
        };
        let me = Value::Object(self.doc.clone());
        match all.iter_mut().find(|r| r["id"] == self.id.as_str()) {
            Some(slot) => *slot = me,
            None => all.push(me),
        }
        let tmp = p.with_extension(format!("{}.tmp", self.id));
        if let Err(e) = std::fs::write(&tmp, Value::Array(all).to_string())
            .and_then(|()| std::fs::rename(&tmp, p))
        {
            eprintln!("observed: {}: {e}", p.display());
        }
    }
}

/// An exclusive OS lock on `<path>.lock`, released when dropped (or when the process dies).
struct Lock(File);

impl Lock {
    fn take(path: &Path) -> Option<Lock> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(format!("{}.lock", path.display()))
            .ok()?;
        let end = Instant::now() + LOCK_WAIT;
        loop {
            match f.try_lock() {
                Ok(()) => return Some(Lock(f)),
                Err(TryLockError::WouldBlock) if Instant::now() < end => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
