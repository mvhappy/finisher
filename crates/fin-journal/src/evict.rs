//! Whole-session eviction (the agent-trace store; sp7 §12.1): when the caps delete a file,
//! every other entry of an agent session it held (`ctx.actor.session`) goes too, oldest first,
//! and later entries of that session are not written, so the store never keeps half a
//! conversation. Off ([`crate::JournalOptions::whole_sessions`]), nothing here runs.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::io::{self, ErrorKind};

use fin_core::events::{Ctx, Envelope};

use crate::control::Pending;
use crate::files::{self, FileMeta};
use crate::writer::Writer;

/// How many evicted sessions the writer remembers (their late entries are dropped).
const REMEMBERED: usize = 4096;

/// The evicted sessions, oldest first, bounded.
#[derive(Debug, Default)]
pub(crate) struct Evicted {
    order: VecDeque<String>,
    set: BTreeSet<String>,
}

impl Evicted {
    fn add(&mut self, s: String) {
        if self.set.insert(s.clone()) {
            self.order.push_back(s);
            while self.order.len() > REMEMBERED {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
        }
    }
}

fn session_of(ctx: Option<&Ctx>) -> Option<&str> {
    ctx?.actor.as_ref()?.session.as_ref().map(|s| s.as_str())
}

impl Writer {
    /// Whether `p` belongs to a session already evicted (it is not written).
    pub(crate) fn evicted(&self, p: &Pending) -> bool {
        self.sh.opts.whole_sessions
            && session_of(p.ctx.as_ref()).is_some_and(|s| self.evicted.set.contains(s))
    }

    /// The agent sessions with entries in `list` (files about to be deleted).
    pub(crate) fn sessions_in(&self, list: &[FileMeta]) -> io::Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for f in list {
            let text = match fs::read(self.dir.join(&f.name)) {
                Ok(t) => t,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for l in text.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                if let Ok(e) = serde_json::from_slice::<Envelope>(l)
                    && let Some(s) = session_of(e.ctx.as_ref())
                {
                    out.insert(s.to_owned());
                }
            }
        }
        Ok(out)
    }

    /// Removes every remaining entry of `sessions` (the caps deleted their oldest files) and
    /// remembers them.
    pub(crate) fn evict_sessions(&mut self, sessions: BTreeSet<String>) -> io::Result<()> {
        if sessions.is_empty() {
            return Ok(());
        }
        for s in &sessions {
            self.evicted.add(s.clone());
        }
        self.flush_buf()?;
        if let Some(f) = self.active.take() {
            files::sync_file(&f)?;
        }
        self.rewrite_each(
            |f| (false, None, f.bytes > 0),
            |e| session_of(e.ctx.as_ref()).is_some_and(|s| sessions.contains(s)),
        )?;
        Ok(())
    }
}
