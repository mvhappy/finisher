//! The file the tests read: flags, environment NAMES (never values), answers, spawns.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

pub struct Observed {
    path: Option<PathBuf>,
    doc: Map<String, Value>,
}

impl Observed {
    pub fn new(path: Option<PathBuf>) -> Self {
        Observed {
            path,
            doc: Map::new(),
        }
    }

    /// Records the invocation: raw argv, and the names of the environment, sorted.
    pub fn start(&mut self, argv: &[String], flags: Value) {
        let mut names: Vec<String> = std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        names.sort();
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
        if let Some(p) = &self.path {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, Value::Object(self.doc.clone()).to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    }
}
