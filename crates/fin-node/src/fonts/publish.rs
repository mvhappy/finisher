//! Publishing an engine's font summary (4C Decision 34): as a status answer (`merged`), with
//! the store's status event (`emit_status`), or as a `status` event of its own when it
//! changed (`refresh`). Each computes, compares and publishes holding the manager's
//! `publish` mutex, so a summary computed earlier is never published after a newer one.

#[cfg(feature = "test-hooks")]
use std::sync::{Arc, mpsc};
#[cfg(feature = "test-hooks")]
use std::time::Duration;
use std::time::Instant;

use fin_store::EngineEvent;
use serde_json::Value;

use super::{Job, Manager, PROGRESS_EVERY, Summary, status};
use crate::engine::lock;
use crate::maintenance::EventSink;

impl Manager {
    /// The summary of an engine's pins now (`None` once detached).
    pub(super) fn summary_of(&self, key: u64) -> Option<Summary> {
        let (needs, unpinned) = {
            let engines = lock(&self.engines);
            let a = engines.get(&key)?;
            (a.needs(), a.unpinned())
        };
        Some(self.summary(&needs, &unpinned))
    }

    /// `raw` (the store's status) with the engine's font summary now merged, which counts
    /// as published.
    pub(super) fn merged(&self, key: u64, raw: Value) -> Value {
        let _publish = lock(&self.publish);
        let Some(s) = self.summary_of(key) else {
            return raw;
        };
        let out = status::merge(raw, &s);
        if let Some(a) = lock(&self.engines).get_mut(&key) {
            a.published = Some((s, Instant::now()));
        }
        out
    }

    /// Emits the store's `status` event `raw` with the engine's last published font summary
    /// (computed now only when none is), under `publish` so it is ordered with the jobs
    /// thread's events. The caller holds the store: nothing is hashed for a summary already
    /// published, and a refresh queued behind it publishes what changed since.
    pub(super) fn emit_status(&self, key: u64, raw: Value, events: &EventSink) {
        let publish = lock(&self.publish);
        let published = lock(&self.engines)
            .get(&key)
            .and_then(|a| a.published.as_ref().map(|(s, _)| s.clone()));
        let s = match published {
            Some(s) => Some(s),
            None => self.summary_of(key).inspect(|s| {
                if let Some(a) = lock(&self.engines).get_mut(&key) {
                    a.published = Some((s.clone(), Instant::now()));
                }
            }),
        };
        let status = match &s {
            Some(s) => status::merge(raw, s),
            None => raw,
        };
        if let Ok(json) = serde_json::to_string(&EngineEvent::Status { status }) {
            events.emit(json);
        }
        drop(publish);
        self.post(Job::Refresh(Some(key)));
    }

    /// Publishes a `status` event when the engine's summary changed since it was published
    /// (progress alone: at most every [`PROGRESS_EVERY`]). Computing, comparing and emitting
    /// all hold `publish`.
    pub(super) fn refresh(&self, key: u64) {
        self.refresh_with(key, || {});
    }

    /// [`Manager::refresh`], running `computed` between computing the summary and comparing
    /// it (test builds pause there).
    fn refresh_with(&self, key: u64, computed: impl FnOnce()) {
        let _publish = lock(&self.publish);
        let Some(s) = self.summary_of(key) else {
            return;
        };
        computed();
        let inner = {
            let mut engines = lock(&self.engines);
            let Some(a) = engines.get_mut(&key) else {
                return;
            };
            let due = match &a.published {
                None => true,
                Some((p, at)) => {
                    p.shape != s.shape
                        || p.warnings != s.warnings
                        || (p.fonts != s.fonts && at.elapsed() >= PROGRESS_EVERY)
                }
            };
            if !due {
                return;
            }
            let Some(inner) = a.inner.upgrade() else {
                return;
            };
            a.published = Some((s.clone(), Instant::now()));
            inner
        };
        let raw = lock(&inner.status).clone();
        if let Some(raw) = raw {
            let status = status::merge(raw, &s);
            if let Ok(json) = serde_json::to_string(&EngineEvent::Status { status }) {
                inner.events.emit(json);
            }
        }
    }

    pub(super) fn refresh_all(&self) {
        let keys: Vec<u64> = lock(&self.engines).keys().copied().collect();
        for key in keys {
            self.refresh(key);
        }
    }

    /// A file's state changed: refresh every engine that pins it.
    pub(super) fn file_changed(&self, sha: &str) {
        let keys: Vec<u64> = lock(&self.engines)
            .iter()
            .filter(|(_, a)| a.all().any(|b| b.needs.iter().any(|n| n.sha256 == sha)))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            self.refresh(key);
        }
    }

    /// Test builds: a refresh computes the summary of the engine's pins now, then (holding
    /// `publish`) waits while `during` changes them (a commit, whose refresh must publish
    /// after it).
    #[cfg(feature = "test-hooks")]
    pub(super) fn publish_race(self: &Arc<Self>, key: u64, during: impl FnOnce()) {
        let (tx, rx) = mpsc::channel();
        if let Some(a) = lock(&self.engines).get_mut(&key) {
            a.published = None;
        }
        let m = self.clone();
        let first = std::thread::spawn(move || {
            m.refresh_with(key, || {
                let _ = tx.send(());
                std::thread::sleep(Duration::from_millis(300));
            });
        });
        let _ = rx.recv();
        during();
        let _ = first.join();
    }
}
