//! The file the tests read: a JSON array with one record per fake process, appended in start
//! order and never removed, so a test can count the processes a resume started. A record holds
//! the flags, environment NAMES (never values), the control traffic, answers and spawns.
//!
//! Several processes may share the file: each update takes `<file>.lock` (created exclusively),
//! rewrites the array with its own record replaced (temp file, then rename) and drops the lock.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

const LOCK_WAIT: Duration = Duration::from_secs(2);

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
        let Some(_lock) = Lock::take(p) else { return };
        let mut all: Vec<Value> = std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let me = Value::Object(self.doc.clone());
        match all.iter_mut().find(|r| r["id"] == self.id.as_str()) {
            Some(slot) => *slot = me,
            None => all.push(me),
        }
        let tmp = p.with_extension(format!("{}.tmp", self.id));
        if std::fs::write(&tmp, Value::Array(all).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

struct Lock(PathBuf);

impl Lock {
    /// Takes `<path>.lock`; a lock older than the wait is a dead process's and is broken.
    fn take(path: &Path) -> Option<Lock> {
        let lock = PathBuf::from(format!("{}.lock", path.display()));
        let end = Instant::now() + LOCK_WAIT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => return Some(Lock(lock)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= end {
                        let _ = std::fs::remove_file(&lock);
                        return OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&lock)
                            .ok()
                            .map(|_| Lock(lock));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
