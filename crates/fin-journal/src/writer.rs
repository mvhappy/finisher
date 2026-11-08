//! The writer thread (plan D, Decisions 16–19): the only owner of `events/`. It drains the
//! control channel before each data batch, serialises entries, rotates files, applies the
//! caps, reports drops, closes stats windows, and suspends on an I/O error or a panic of its
//! own instead of failing anyone. The file side (scan, rotation, caps, suspension) is in
//! `disk.rs`.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use fin_core::events::payloads::JournalDropped;
use fin_core::events::{AnyPayload, CollectPolicy, Purpose, Purposes, Source};
use serde_json::{Map, Value};
use ulid::Ulid;

use crate::blobs::Blobs;
use crate::consent::ConsentState;
use crate::control::{Control, Data, Pending, SAMPLE_BYTES, gate_of, unpack};
use crate::files::FileMeta;
use crate::stats::Windows;
use crate::{JournalError, MAX_ENTRY_BYTES, Shared, encode};

/// What the writer starts from: the consent in force and whether `state.json` needs it, and
/// the stored state's other keys.
pub(crate) struct Init {
    pub state: ConsentState,
    pub dirty: bool,
    pub other: Map<String, Value>,
}

thread_local! {
    pub(crate) static ON_WRITER: Cell<bool> = const { Cell::new(false) };
}

/// How long a suspended journal waits before it tries the disk again.
pub(crate) const RETRY_MS: u64 = 60_000;
pub(crate) const CAPS_EVERY_MS: u64 = 3_600_000;
/// Data messages handled per batch; control is drained between batches.
const BATCH: usize = 256;

pub(crate) struct Suspension {
    pub code: &'static str,
    pub retry_at: u64,
}

/// One buffered line: where it starts in the buffer and the purposes it was encoded under.
/// An `exempt` line keeps its stamp whatever the gate is now (`consent.changed`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line {
    pub start: usize,
    pub purposes: Purposes,
    pub exempt: bool,
}

pub(crate) struct Writer {
    pub sh: Arc<Shared>,
    pub dir: PathBuf,
    pub data: Receiver<Data>,
    pub ctl: Receiver<(u64, Control)>,
    /// A control message read ahead (`control_due`), handled before the data after it.
    pub next_ctl: Option<(u64, Control)>,
    /// Data messages handled (compared with a control message's count).
    pub done: u64,
    /// Oldest first; with `active`, the last one is being appended to.
    pub files: Vec<FileMeta>,
    pub active: Option<File>,
    pub buf: Vec<u8>,
    pub lines: Vec<Line>,
    scratch: Vec<u8>,
    pub last_flush: Instant,
    /// The directory was listed, the newest file's tail repaired and the caps applied.
    pub scanned: bool,
    /// The directory was listed without a repair (opened off): the caps may delete.
    pub listed: bool,
    pub suspended: Option<Suspension>,
    pub windows: Windows,
    pub next_caps: u64,
    pub last_id: Ulid,
    drops_since: u64,
    last_drops: Option<Instant>,
    pub blobs: Blobs,
    /// The bytes of the blobs the active file references (per reference).
    pub active_blob_bytes: u64,
    /// What this server knows of each project's flag (`None`: unknown).
    pub policies: BTreeMap<String, Option<CollectPolicy>>,
    /// Projects set to `never` whose purge failed: retried until it succeeds.
    pub pending_purges: std::collections::BTreeSet<String>,
    /// The consent `state.json` must hold, and whether it does not yet.
    pub state: ConsentState,
    pub state_dirty: bool,
    pub state_other: Map<String, Value>,
    pub session: crate::session::SessionState,
    /// Sessions whole-session eviction removed (`evict.rs`).
    pub evicted: crate::evict::Evicted,
}

impl Writer {
    pub fn new(
        sh: Arc<Shared>,
        dir: PathBuf,
        data: Receiver<Data>,
        ctl: Receiver<(u64, Control)>,
        init: Init,
    ) -> Writer {
        let now = sh.opts.clock.now_ms();
        Writer {
            sh,
            dir,
            data,
            ctl,
            next_ctl: None,
            done: 0,
            files: Vec::new(),
            active: None,
            buf: Vec::new(),
            lines: Vec::new(),
            scratch: Vec::new(),
            last_flush: Instant::now(),
            scanned: false,
            listed: false,
            suspended: None,
            windows: Windows::default(),
            next_caps: now,
            last_id: Ulid::nil(),
            drops_since: now,
            last_drops: None,
            blobs: Blobs::default(),
            active_blob_bytes: 0,
            policies: BTreeMap::new(),
            pending_purges: Default::default(),
            state: init.state,
            state_dirty: init.dirty,
            state_other: init.other,
            session: Default::default(),
            evicted: Default::default(),
        }
    }

    pub fn now(&self) -> u64 {
        self.sh.opts.clock.now_ms()
    }

    /// The purposes in force now.
    pub fn gate(&self) -> Purposes {
        gate_of(self.sh.word.load(Acquire))
    }

    pub fn run(mut self) {
        ON_WRITER.with(|w| w.set(true));
        encode::warm();
        if self.gate().is_empty() {
            self.list_only();
        } else if let Err(e) = self.scan() {
            self.suspend(&e);
        }
        loop {
            match catch_unwind(AssertUnwindSafe(|| self.step())) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => self.on_panic(),
            }
        }
    }

    /// One batch of data, or (none queued) one wait. False once the journal is closed.
    ///
    /// The writer parks rather than blocking on the data channel, so a sender never wakes
    /// it for one entry ([`crate::handle::Handle::send`]): data waits for the flush timer
    /// ([`Writer::wait`]) unless enough of it is in flight, and every control message wakes
    /// the writer at once.
    fn step(&mut self) -> bool {
        self.sh.faults.park_if_paused();
        if !self.drain_control() {
            return false;
        }
        let mut taken = 0;
        while taken < BATCH {
            match self.data.try_recv() {
                Ok(d) => {
                    if !self.take_data(d) {
                        return false;
                    }
                    taken += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.drain_control();
                    self.shutdown();
                    return false;
                }
            }
        }
        if taken == 0 {
            std::thread::park_timeout(self.wait());
        }
        self.tick();
        true
    }

    /// How long to park with nothing queued: until the buffer's flush is due, else one flush
    /// interval (at most 1 s), when queued data is taken.
    fn wait(&self) -> Duration {
        let idle = Duration::from_secs(1);
        if self.buf.is_empty() {
            return self
                .sh
                .opts
                .flush_every
                .clamp(Duration::from_millis(1), idle);
        }
        let every = self.sh.opts.flush_every;
        every
            .saturating_sub(self.last_flush.elapsed())
            .clamp(Duration::from_millis(1), idle)
    }

    /// One data message; a panic while handling it suspends the journal here, so a control
    /// message being handled still answers (with the suspension).
    pub fn on_data(&mut self, d: Data) {
        self.done += 1;
        if catch_unwind(AssertUnwindSafe(|| self.handle_data(d))).is_err() {
            self.on_panic();
        }
    }

    fn handle_data(&mut self, d: Data) {
        // The bytes leave the bound before anything can panic, so a panic does not leak them.
        match &d {
            Data::Entry(p) => self.sh.inflight.fetch_sub(p.bytes, Relaxed),
            Data::Sample(_) => self.sh.inflight.fetch_sub(SAMPLE_BYTES, Relaxed),
            Data::Blob(b) => self.sh.inflight.fetch_sub(b.bytes.len(), Relaxed),
            Data::Wake => 0,
        };
        self.sh.faults.check_panic();
        match d {
            Data::Entry(p) => {
                self.write(*p);
            }
            Data::Sample(s) => {
                self.roll_stats(s.ts);
                if !self.gate().is_empty() {
                    self.windows.add(&s, self.stats_ms());
                }
            }
            Data::Blob(b) => {
                if self.gate().has_training() {
                    self.blobs.offer(*b);
                }
            }
            Data::Wake => {}
        }
    }

    fn tick(&mut self) {
        self.maintain();
        if !self.buf.is_empty() && self.last_flush.elapsed() >= self.sh.opts.flush_every {
            let _ = self.flush();
        }
    }

    /// What is due by the clock: a suspension's retry, the stats window, the drop report,
    /// the hourly caps (which delete old files even while off).
    pub fn maintain(&mut self) {
        let now = self.now();
        self.retry_due(now);
        self.blobs.evict(Instant::now());
        self.roll_stats(now);
        self.report_drops(false);
        if (self.scanned || self.listed)
            && self.suspended.is_none()
            && now >= self.next_caps
            && let Err(e) = self.apply_caps(now)
        {
            self.suspend(&e);
        }
        self.retry_purges();
    }

    pub fn stats_ms(&self) -> u64 {
        (self.sh.opts.stats_every.as_millis() as u64).max(1)
    }

    fn roll_stats(&mut self, now: u64) {
        if self
            .windows
            .due_at(self.stats_ms())
            .is_some_and(|due| now >= due)
        {
            for (source, p) in self.windows.take(None, self.stats_ms()) {
                self.write_own(source, p, now);
            }
        }
    }

    /// An entry of the journal's own (stats, drops), under the consent in force now.
    pub fn write_own(&mut self, source: Source, payload: AnyPayload, ts: u64) -> bool {
        let stamp = unpack(self.sh.word.load(Acquire));
        let bytes = encode::entry_bytes(None, &payload);
        self.write(Pending {
            ts,
            session: None,
            source,
            ctx: None,
            payload,
            stamp,
            bytes,
        })
    }

    /// `journal.dropped`, at most once per flush interval unless `force` (close).
    pub fn report_drops(&mut self, force: bool) {
        if self.suspended.is_some()
            || self.gate().is_empty()
            || self.sh.drop_count.load(Relaxed) == 0
            || (!force
                && self
                    .last_drops
                    .is_some_and(|t| t.elapsed() < self.sh.opts.flush_every))
        {
            return;
        }
        self.last_drops = Some(Instant::now());
        let count = self.sh.drop_count.swap(0, Relaxed);
        let bytes = self.sh.drop_bytes.swap(0, Relaxed);
        let now = self.now();
        let since = std::mem::replace(&mut self.drops_since, now);
        let p = JournalDropped {
            count,
            bytes,
            since,
        };
        self.write_own(Source::FinNode, p.into(), now);
    }

    /// Serialises and buffers one entry under the stamp ∩ the gate, unless its project may
    /// not be recorded (`never` or unknown: not a drop). False when it was not written.
    pub fn write(&mut self, p: Pending) -> bool {
        if let Some(project) = p.ctx.as_ref().and_then(|c| c.project.as_ref())
            && !self.project_allowed(project.as_str())
        {
            if self
                .policies
                .get(project.as_str())
                .copied()
                .flatten()
                .is_none()
            {
                self.sh.skipped_unknown.fetch_add(1, Relaxed);
            }
            return false;
        }
        if self.evicted(&p) {
            return false;
        }
        let purposes = p.stamp.purposes.intersect(self.gate());
        self.write_with(p, purposes, false)
    }

    /// [`Writer::write`] with the purposes given (an `exempt` line keeps them whatever the
    /// gate becomes). Blobs the entry references are written first; one whose bytes never
    /// arrived takes the entry's content with it. False (and counted) when not written.
    pub fn write_with(&mut self, mut p: Pending, purposes: Purposes, exempt: bool) -> bool {
        let bytes = p.bytes;
        let now = self.now();
        if purposes.is_empty() || !self.writable(now) {
            self.sh.dropped(bytes);
            return false;
        }
        if purposes != p.stamp.purposes && !exempt {
            // A purpose went since it was recorded: the basis is the one in force now.
            p.stamp.basis = unpack(self.sh.word.load(Acquire)).basis;
        }
        let mut blobs: Vec<String> = Vec::new();
        if purposes.contains(Purpose::Training) {
            blobs = p.payload.blobs().map(|b| b.hash.hex().to_owned()).collect();
            match self.blobs.ensure(&self.dir, &blobs) {
                Ok(true) => {}
                Ok(false) => {
                    p.payload.strip_content();
                    blobs.clear();
                }
                Err(e) => {
                    self.suspend(&e);
                    self.sh.dropped(bytes);
                    return false;
                }
            }
        }
        let ts = p.ts;
        let id = self.next_id(ts);
        let mut line = std::mem::take(&mut self.scratch);
        line.clear();
        let ok = encode::encode(&mut line, id, &self.sh.session, p, purposes).is_ok()
            && line.len() <= MAX_ENTRY_BYTES;
        self.blobs.pin(&blobs);
        let written = ok && self.append(ts, &line, purposes, exempt);
        self.blobs.unpin();
        self.scratch = line;
        if !written {
            self.sh.dropped(bytes);
        } else if !blobs.is_empty()
            && let Some(m) = self.files.last()
        {
            let name = m.name.clone();
            self.active_blob_bytes += blobs.iter().map(|h| self.blobs.size_of(h)).sum::<u64>();
            self.blobs.add(&name, &blobs);
        }
        written
    }

    pub fn append(&mut self, ts: u64, line: &[u8], purposes: Purposes, exempt: bool) -> bool {
        if let Err(e) = self.prepare(ts, line.len() as u64) {
            self.suspend(&e);
            return false;
        }
        self.lines.push(Line {
            start: self.buf.len(),
            purposes,
            exempt,
        });
        self.buf.extend_from_slice(line);
        if let Some(m) = self.files.last_mut() {
            m.bytes += line.len() as u64;
            m.end_ts = m.end_ts.max(ts);
        }
        if self.buf.len() >= self.sh.opts.flush_bytes {
            let _ = self.flush();
        }
        true
    }

    /// Clears an expired suspension; false while one holds.
    fn writable(&mut self, now: u64) -> bool {
        match &self.suspended {
            Some(s) if now < s.retry_at => false,
            Some(_) => {
                self.suspended = None;
                self.scanned = false;
                self.publish();
                true
            }
            None => true,
        }
    }

    /// A due retry from `maintain`: with a purpose on, the directory is scanned again (which
    /// creates nothing), so the status recovers without waiting for an entry.
    fn retry_due(&mut self, now: u64) {
        if self.suspended.as_ref().is_none_or(|s| now < s.retry_at) {
            return;
        }
        self.suspended = None;
        self.scanned = false;
        if self.gate().is_empty() {
            // Off: only what deletes runs (the caps, a pending `never` purge).
            self.listed = false;
            self.list_only();
        } else if let Err(e) = self.scan() {
            self.suspend(&e);
        }
        self.publish();
    }

    pub fn next_id(&mut self, ts: u64) -> Ulid {
        let fresh = Ulid::from_parts(ts, Ulid::new().random());
        let id = if fresh <= self.last_id {
            self.last_id.increment().unwrap_or(fresh)
        } else {
            fresh
        };
        self.last_id = id;
        id
    }

    pub fn flush_buf(&mut self) -> io::Result<()> {
        self.enforce_gate();
        if self.buf.is_empty() {
            self.lines.clear();
            return Ok(());
        }
        self.persist_meta()?;
        self.sh.faults.before_write()?;
        let f = self
            .active
            .as_mut()
            .ok_or_else(|| io::Error::other("no active journal file"))?;
        if let Some(n) = self.sh.faults.tear_at(&self.buf) {
            f.write_all(&self.buf[..n])?;
            self.sh.faults.park_torn();
        }
        f.write_all(&self.buf)?;
        self.buf.clear();
        self.lines.clear();
        self.last_flush = Instant::now();
        self.publish();
        Ok(())
    }

    /// Writes the buffer out (no fsync: a power loss may lose the last flush interval).
    pub fn flush(&mut self) -> Result<(), JournalError> {
        if let Err(e) = self.flush_buf() {
            self.suspend(&e);
        }
        match &self.suspended {
            Some(s) => Err(JournalError::new(
                "JOURNAL_UNAVAILABLE",
                format!("the journal is suspended ({})", s.code),
            )),
            None => Ok(()),
        }
    }
}
