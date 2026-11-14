//! `events/state.json` (plan D, Decisions 12 and 25): the effective consent with its
//! monotonic revision, written temp-then-rename by the writer before the first line it
//! covers, so it exists only once something was recorded. Keys of a newer journal (D4's
//! `openSession`) are kept on rewrite.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use fin_core::events::payloads::ConsentChanged;
use fin_core::events::{AnyPayload, Source};

use crate::consent::ConsentState;
use crate::control::{Pending, Stamp, unpack};
use crate::encode;
use crate::key;
use crate::writer::Line;
use crate::writer::Writer;
use std::sync::atomic::Ordering::Acquire;

pub(crate) const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StateFile {
    pub consent: ConsentState,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

/// The state file in `dir`, if there is one that parses. Reads only.
pub(crate) fn read(dir: &Path) -> Option<StateFile> {
    let bytes = std::fs::read(dir.join(STATE_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write(dir: &Path, s: &StateFile) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    key::write_private(dir, STATE_FILE, &bytes)
}

/// The revision of `now` given what `stored` recorded: the same decision keeps the stored
/// revision, a different one is the next; never below `floor` (the caller's own count).
pub(crate) fn reconcile(stored: Option<&ConsentState>, now: &ConsentState, floor: u64) -> u64 {
    match stored {
        None => floor,
        Some(s) if s.same(now) => s.rev.max(floor),
        Some(s) => (s.rev + 1).max(floor),
    }
}

impl Writer {
    /// The key and `state.json`, before the first line that needs them.
    pub fn persist_meta(&mut self) -> std::io::Result<()> {
        if let Some(k) = self.sh.key()
            && !k.persisted()
        {
            k.persist(&self.dir)?;
        }
        if self.state_dirty {
            crate::files::ensure_dir(&self.dir)?;
            let s = StateFile {
                consent: self.state,
                other: self.state_other.clone(),
            };
            write(&self.dir, &s)?;
            self.state_dirty = false;
        }
        Ok(())
    }

    /// A consent change (Decision 12): the gate word holds `to` already, and the entries
    /// recorded before were written under it (the control channel catches up first).
    /// `consent.changed` goes in under the old consent (the new one when the old was off),
    /// whatever the new gate allows; the buffered lines are re-checked; then a flush.
    pub fn on_consent(&mut self, from: ConsentState, to: ConsentState) {
        self.state = to;
        self.state_dirty = true;
        let (old, new) = (from.purposes(), to.purposes());
        if !old.is_empty() || !new.is_empty() {
            let (by, stamp) = if old.is_empty() {
                (to, new)
            } else {
                (from, old)
            };
            let payload: AnyPayload = ConsentChanged {
                from: old,
                to: new,
                basis: to.basis,
                rev: to.rev,
            }
            .into();
            let bytes = crate::encode::entry_bytes(None, &payload);
            let p = Pending {
                ts: self.now(),
                session: None,
                source: Source::FinNode,
                ctx: None,
                payload,
                stamp: Stamp {
                    purposes: stamp,
                    rev: by.rev,
                    basis: by.basis,
                },
                bytes,
            };
            self.write_with(p, stamp, true);
        }
        if old.is_empty() && !new.is_empty() {
            self.session_turned_on();
        }
        self.enforce_gate();
        let _ = self.flush();
        if from.purposes().has_training() && !new.has_training() {
            // Training went: what the stripped lines referenced is collected now.
            self.blobs.drop_pending();
            if let Err(e) = self.blobs.gc(&self.dir) {
                self.suspend(&e);
            }
            self.publish();
        }
    }

    /// Rewrites the buffered lines the gate no longer allows as they stand: content goes
    /// with training, and a line left with no purpose is dropped (and counted). So what
    /// reaches the disk never holds more than the consent in force when it is written.
    pub fn enforce_gate(&mut self) {
        let word = unpack(self.sh.word.load(Acquire));
        let gate = word.purposes;
        let allowed = |l: &Line| l.exempt || l.purposes.intersect(gate) == l.purposes;
        if self.lines.iter().all(allowed) {
            return;
        }
        let old = std::mem::take(&mut self.buf);
        let lines = std::mem::take(&mut self.lines);
        for (i, l) in lines.iter().enumerate() {
            let end = lines.get(i + 1).map_or(old.len(), |n| n.start);
            let text = &old[l.start..end];
            if allowed(l) {
                self.lines.push(Line {
                    start: self.buf.len(),
                    ..*l
                });
                self.buf.extend_from_slice(text);
                continue;
            }
            let (kept, gone) = encode::restamp(text, gate, word.basis);
            if let Some(m) = self.files.last() {
                let name = m.name.clone();
                let gone: Vec<String> = gone.iter().map(|b| b.hash.hex().to_owned()).collect();
                self.blobs.unref(&name, &gone);
            }
            match kept {
                Some(t) => {
                    self.lines.push(Line {
                        start: self.buf.len(),
                        purposes: l.purposes.intersect(gate),
                        exempt: false,
                    });
                    self.buf.extend_from_slice(&t);
                }
                None => self.sh.dropped(text.len()),
            }
        }
        if let Some(m) = self.files.last_mut() {
            m.bytes = (m.bytes + self.buf.len() as u64).saturating_sub(old.len() as u64);
        }
    }
}
