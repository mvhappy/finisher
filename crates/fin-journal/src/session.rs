//! Sessions (plan D, Decision 25): `session.start` when the journal is on (it opened with a
//! purpose on, or one turned on), `session.end` on a clean shutdown, and `session.unclean`
//! when a start finds the previous run still open in `state.json` (`openSession`, kept
//! beside the consent). The writer owns `state.json`, so the bookkeeping is here.

use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use fin_core::events::payloads::{SessionEnd, SessionStart, SessionUnclean};
use fin_core::events::{IdRef, Source};
use serde_json::Value;

use crate::Journal;
use crate::control::Control;
use crate::key;
use crate::writer::Writer;

/// `state.json`'s key for the run that has not ended yet.
pub(crate) const OPEN_SESSION: &str = "openSession";

/// What the server tells the writer about its run.
pub(crate) enum SessionMsg {
    /// The run's platform facts: `session.start` now if on, else when a purpose turns on;
    /// then the ack (`session.start` and `openSession` are on disk, or nothing is due).
    Start {
        start: Box<SessionStart>,
        ack: SyncSender<bool>,
    },
    /// A clean shutdown: `session.end` if on, `openSession` cleared; then the ack.
    End {
        uptime_s: u64,
        ack: SyncSender<bool>,
    },
}

/// The writer's view of the run.
#[derive(Debug, Default)]
pub(crate) struct SessionState {
    start: Option<SessionStart>,
    /// `session.start` was written for this run (and `openSession` names it).
    open: bool,
}

impl Writer {
    pub fn on_session(&mut self, m: SessionMsg) {
        match m {
            SessionMsg::Start { start, ack } => {
                self.session.start = Some(*start);
                // The consent as of this message (`state`, which consent changes update in
                // channel order), not the gate word: a change sent after this message has
                // already stored its gate, and its `consent.changed` starts the run itself.
                let on = !self.state.purposes().is_empty();
                let ok = !on || self.begin_session();
                let _ = ack.try_send(ok);
            }
            SessionMsg::End { uptime_s, ack } => {
                let ok = self.end_session(uptime_s);
                let _ = ack.try_send(ok);
            }
        }
    }

    /// A purpose turned on (`on_consent`): the run starts now, if the server announced it.
    pub fn session_turned_on(&mut self) {
        if self.session.start.is_some() {
            let _ = self.begin_session();
        }
    }

    /// `session.unclean` for a previous run still open, `session.start`, and `openSession`
    /// set to this run (persisted with the first line).
    fn begin_session(&mut self) -> bool {
        let Some(start) = self.session.start.clone() else {
            return false;
        };
        let own = self.sh.session.as_str().to_owned();
        let previous = self
            .state_other
            .get(OPEN_SESSION)
            .and_then(Value::as_str)
            .filter(|p| *p != own)
            .map(str::to_owned);
        let now = self.now();
        if let Some(prev) = previous {
            let id = IdRef::classify(&prev)
                .ok()
                .or_else(|| self.sh.hasher().map(|k| key::id_ref(&k, &prev)));
            if let Some(previous) = id {
                self.write_own(Source::Finisherd, SessionUnclean { previous }.into(), now);
            }
        }
        let written = self.write_own(Source::Finisherd, start.into(), now);
        if written {
            self.session.open = true;
            self.state_other
                .insert(OPEN_SESSION.to_owned(), Value::String(own));
            self.state_dirty = true;
        }
        // The flush writes `state.json` (with `openSession`) before the line.
        self.flush().is_ok() && written
    }

    /// `session.end` (when on and started) and `openSession` cleared, flushed. True when
    /// `session.end` is on disk.
    fn end_session(&mut self, uptime_s: u64) -> bool {
        let mut ok = false;
        if self.session.open && !self.gate().is_empty() {
            let now = self.now();
            ok = self.write_own(Source::Finisherd, SessionEnd { uptime_s }.into(), now);
        }
        let own = self.sh.session.as_str();
        let ours = self.state_other.get(OPEN_SESSION).and_then(Value::as_str) == Some(own);
        if ours {
            self.state_other.remove(OPEN_SESSION);
            self.state_dirty = true;
        }
        self.session.open = false;
        ok = self.flush().is_ok() && ok;
        if self.state_dirty && ours && self.persist_meta().is_err() {
            return false;
        }
        ok
    }
}

impl Journal {
    /// Announces this run (Decision 25) with its platform facts. The writer journals
    /// `session.start` (after `session.unclean` when `state.json` still names an earlier
    /// run) now if a purpose is on, else when one turns on. Waits at most `timeout` for the
    /// writer: true when the start and `openSession` are on disk, or nothing was due (off).
    /// A server announces before it reports ready, so a kill right after cannot lose them.
    pub fn start_session(&self, start: SessionStart, timeout: Duration) -> bool {
        let Some(h) = &self.h else { return false };
        if Journal::on_writer_thread() {
            return false;
        }
        let (ack, rx) = mpsc::sync_channel(1);
        let msg = SessionMsg::Start {
            start: Box::new(start),
            ack,
        };
        if !h.control(Control::Session(msg)) {
            return false;
        }
        rx.recv_timeout(timeout).unwrap_or(false)
    }

    /// A clean shutdown: journals `session.end` (when the run started) and clears
    /// `openSession`, flushed, waiting at most `timeout` (1 s in the server). True when
    /// `session.end` is on disk. Call it before [`Journal::close`], off the JS thread.
    pub fn end_session(&self, uptime_s: u64, timeout: Duration) -> bool {
        let Some(h) = &self.h else { return false };
        if Journal::on_writer_thread() {
            return false;
        }
        let (ack, rx) = mpsc::sync_channel(1);
        if !h.control(Control::Session(SessionMsg::End { uptime_s, ack })) {
            return false;
        }
        rx.recv_timeout(timeout).unwrap_or(false)
    }
}
