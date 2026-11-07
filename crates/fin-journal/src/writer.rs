//! The writer thread (plan D, Decisions 16–19): the only owner of `events/`. It drains the
//! control channel before each data batch, serialises entries, rotates files, applies the
//! caps, reports drops, closes stats windows, and suspends on an I/O error or a panic of its
//! own instead of failing anyone.

use std::cell::Cell;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use fin_core::events::payloads::JournalDropped;
use fin_core::events::{AnyPayload, Source};
use ulid::Ulid;

use crate::control::{Control, Data, Pending, SAMPLE_BYTES, gate_of, unpack};
use crate::files::{self, DAY_MS, FileMeta};
use crate::stats::Windows;
use crate::{JournalError, MAX_ENTRY_BYTES, Shared, encode};

thread_local! {
    pub(crate) static ON_WRITER: Cell<bool> = const { Cell::new(false) };
}

/// How long a suspended journal waits before it tries the disk again.
pub(crate) const RETRY_MS: u64 = 60_000;
const CAPS_EVERY_MS: u64 = 3_600_000;
/// Data messages handled per batch; control is drained between batches.
const BATCH: usize = 256;

struct Suspension {
    code: &'static str,
    retry_at: u64,
}

pub(crate) struct Writer {
    sh: Arc<Shared>,
    dir: PathBuf,
    data: Receiver<Data>,
    ctl: Receiver<Control>,
    /// Oldest first; with `active`, the last one is being appended to.
    files: Vec<FileMeta>,
    active: Option<File>,
    buf: Vec<u8>,
    buf_entries: u64,
    scratch: Vec<u8>,
    last_flush: Instant,
    /// The directory was listed, the newest file's tail repaired and the caps applied.
    scanned: bool,
    suspended: Option<Suspension>,
    windows: Windows,
    next_caps: u64,
    last_id: Ulid,
    drops_since: u64,
}

fn code_of(e: &io::Error) -> &'static str {
    match e.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => "JOURNAL_DISK_FULL",
        ErrorKind::PermissionDenied
        | ErrorKind::ReadOnlyFilesystem
        | ErrorKind::NotADirectory
        | ErrorKind::AlreadyExists => "JOURNAL_UNWRITABLE",
        _ => "JOURNAL_IO",
    }
}

impl Writer {
    pub fn new(
        sh: Arc<Shared>,
        dir: PathBuf,
        data: Receiver<Data>,
        ctl: Receiver<Control>,
    ) -> Writer {
        let now = sh.opts.clock.now_ms();
        Writer {
            sh,
            dir,
            data,
            ctl,
            files: Vec::new(),
            active: None,
            buf: Vec::new(),
            buf_entries: 0,
            scratch: Vec::new(),
            last_flush: Instant::now(),
            scanned: false,
            suspended: None,
            windows: Windows::default(),
            next_caps: now,
            last_id: Ulid::nil(),
            drops_since: now,
        }
    }

    fn now(&self) -> u64 {
        self.sh.opts.clock.now_ms()
    }

    pub fn run(mut self) {
        ON_WRITER.with(|w| w.set(true));
        encode::warm();
        if gate_of(self.sh.word.load(Relaxed)).is_empty() {
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

    /// One wait for data and its batch. False once the journal is closed.
    fn step(&mut self) -> bool {
        self.sh.faults.park_if_paused();
        if !self.drain_control() {
            return false;
        }
        match self.data.recv_timeout(self.wait()) {
            Ok(d) => {
                self.on_data(d);
                for i in 1..BATCH {
                    if i % 64 == 0 && !self.drain_control() {
                        return false;
                    }
                    match self.data.try_recv() {
                        Ok(d) => self.on_data(d),
                        Err(_) => break,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                self.drain_control();
                self.shutdown();
                return false;
            }
        }
        self.tick();
        true
    }

    fn wait(&self) -> Duration {
        let idle = Duration::from_secs(1);
        if self.buf.is_empty() {
            return idle;
        }
        let every = self.sh.opts.flush_every;
        every
            .saturating_sub(self.last_flush.elapsed())
            .clamp(Duration::from_millis(1), idle)
    }

    /// Handles the data queued so far (at most a channel's worth), so a control message
    /// sees every entry recorded before it.
    fn catch_up(&mut self) {
        for _ in 0..self.sh.opts.channel.max(1) {
            match self.data.try_recv() {
                Ok(d) => self.on_data(d),
                Err(_) => break,
            }
        }
    }

    fn drain_control(&mut self) -> bool {
        while let Ok(c) = self.ctl.try_recv() {
            self.catch_up();
            match c {
                Control::Sync { entry, slot, ack } => {
                    if slot.take() {
                        let ok = self.write(*entry) && self.flush().is_ok();
                        let _ = ack.try_send(ok);
                    }
                }
                Control::Flush(ack) => {
                    self.maintain();
                    let _ = ack.try_send(self.flush());
                }
                Control::Close(ack) => {
                    self.shutdown();
                    let _ = ack.try_send(());
                    return false;
                }
            }
        }
        true
    }

    /// One data message; a panic while handling it suspends the journal here, so a control
    /// message being handled still answers (with the suspension).
    fn on_data(&mut self, d: Data) {
        if catch_unwind(AssertUnwindSafe(|| self.handle_data(d))).is_err() {
            self.on_panic();
        }
    }

    fn handle_data(&mut self, d: Data) {
        self.sh.faults.check_panic();
        match d {
            Data::Entry(p) => {
                self.sh.inflight.fetch_sub(p.bytes, Relaxed);
                self.write(*p);
            }
            Data::Sample(s) => {
                self.sh.inflight.fetch_sub(SAMPLE_BYTES, Relaxed);
                self.roll_stats(s.ts);
                if !gate_of(self.sh.word.load(Relaxed)).is_empty() {
                    self.windows.add(&s, self.stats_ms());
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

    /// What is due by the clock: the stats window, the drop report, the hourly caps.
    fn maintain(&mut self) {
        let now = self.now();
        self.roll_stats(now);
        self.report_drops();
        if self.scanned
            && self.suspended.is_none()
            && now >= self.next_caps
            && let Err(e) = self.apply_caps(now)
        {
            self.suspend(&e);
        }
    }

    fn stats_ms(&self) -> u64 {
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
    fn write_own(&mut self, source: Source, payload: AnyPayload, ts: u64) -> bool {
        let stamp = unpack(self.sh.word.load(Relaxed));
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

    fn report_drops(&mut self) {
        if self.suspended.is_some()
            || gate_of(self.sh.word.load(Relaxed)).is_empty()
            || self.sh.drop_count.load(Relaxed) == 0
        {
            return;
        }
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

    /// Serialises and buffers one entry. False (and counted as dropped) when it was not.
    fn write(&mut self, p: Pending) -> bool {
        let bytes = p.bytes;
        let purposes = p
            .stamp
            .purposes
            .intersect(gate_of(self.sh.word.load(Relaxed)));
        let now = self.now();
        if purposes.is_empty() || !self.writable(now) {
            self.sh.dropped(bytes);
            return false;
        }
        let ts = p.ts;
        let id = self.next_id(ts);
        let mut line = std::mem::take(&mut self.scratch);
        line.clear();
        let ok = encode::encode(&mut line, id, &self.sh.session, p, purposes).is_ok()
            && line.len() <= MAX_ENTRY_BYTES;
        let written = ok && self.append(ts, &line);
        self.scratch = line;
        if !written {
            self.sh.dropped(bytes);
        }
        written
    }

    fn append(&mut self, ts: u64, line: &[u8]) -> bool {
        if let Err(e) = self.prepare(ts, line.len() as u64) {
            self.suspend(&e);
            return false;
        }
        self.buf.extend_from_slice(line);
        self.buf_entries += 1;
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

    fn next_id(&mut self, ts: u64) -> Ulid {
        let fresh = Ulid::from_parts(ts, Ulid::new().random());
        let id = if fresh <= self.last_id {
            self.last_id.increment().unwrap_or(fresh)
        } else {
            fresh
        };
        self.last_id = id;
        id
    }

    /// Makes the active file ready for a `len`-byte line at `ts`: the first write scans the
    /// directory and creates it; a full file or a new UTC day rotates.
    fn prepare(&mut self, ts: u64, len: u64) -> io::Result<()> {
        if !self.scanned {
            self.scan()?;
        }
        if self.active.is_some()
            && let Some(m) = self.files.last()
            && ((m.bytes > 0 && m.bytes + len > self.sh.opts.max_file_bytes) || ts / DAY_MS > m.day)
        {
            self.rotate()?;
        }
        if self.active.is_none() {
            files::ensure_dir(&self.dir)?;
            let id = self.next_id(ts);
            let day = ts / DAY_MS;
            let name = files::file_name(day, id);
            let f = files::create(&self.dir.join(&name))?;
            files::sync_dir(&self.dir)?;
            self.files.push(FileMeta {
                name,
                id,
                day,
                start_ts: ts,
                end_ts: ts,
                bytes: 0,
            });
            self.active = Some(f);
            self.publish();
        }
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush_buf()?;
        if let Some(f) = self.active.take() {
            files::sync_file(&f)?;
        }
        let now = self.now();
        self.apply_caps(now)
    }

    /// Lists the existing files, repairs the newest one's tail and applies the caps. Creates
    /// nothing: a missing directory is an empty journal.
    fn scan(&mut self) -> io::Result<()> {
        self.active = None;
        self.files.clear();
        let list = files::list(&self.dir)?;
        let newest = list.len().checked_sub(1);
        for (i, (name, id, len)) in list.into_iter().enumerate() {
            let path = self.dir.join(&name);
            let len = if Some(i) == newest {
                files::repair_tail(&path, len)?
            } else {
                len
            };
            let start_ts = id.timestamp_ms();
            let end_ts = files::last_ts(&path, len).unwrap_or(start_ts);
            self.last_id = self.last_id.max(id);
            self.files.push(FileMeta {
                name,
                id,
                day: start_ts / DAY_MS,
                start_ts,
                end_ts,
                bytes: len,
            });
        }
        self.scanned = true;
        let now = self.now();
        self.apply_caps(now)
    }

    /// The status of a journal opened off: what is on disk, touching nothing.
    fn list_only(&mut self) {
        if let Ok(list) = files::list(&self.dir) {
            self.files = list
                .into_iter()
                .map(|(name, id, len)| FileMeta {
                    name,
                    id,
                    day: id.timestamp_ms() / DAY_MS,
                    start_ts: id.timestamp_ms(),
                    end_ts: id.timestamp_ms(),
                    bytes: len,
                })
                .collect();
        }
        self.publish();
    }

    fn apply_caps(&mut self, now: u64) -> io::Result<()> {
        let o = &self.sh.opts;
        let max_age = o.max_age.as_millis() as u64;
        let n = files::over_caps(
            &self.files,
            self.active.is_some(),
            now,
            max_age,
            o.max_total_bytes,
        );
        let mut result = Ok(());
        let mut removed = 0;
        for f in &self.files[..n] {
            match fs::remove_file(self.dir.join(&f.name)) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == ErrorKind::NotFound => removed += 1,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.files.drain(..removed);
        if removed > 0 {
            result = result.and(files::sync_dir(&self.dir));
        }
        self.next_caps = now + CAPS_EVERY_MS;
        self.publish();
        result
    }

    fn flush_buf(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.sh.faults.before_write()?;
        let f = self
            .active
            .as_mut()
            .ok_or_else(|| io::Error::other("no active journal file"))?;
        f.write_all(&self.buf)?;
        self.buf.clear();
        self.buf_entries = 0;
        self.last_flush = Instant::now();
        self.publish();
        Ok(())
    }

    /// Writes the buffer out (no fsync: a power loss may lose the last flush interval).
    fn flush(&mut self) -> Result<(), JournalError> {
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

    fn suspend_with(&mut self, code: &'static str) {
        self.active = None;
        if self.buf_entries > 0 {
            self.sh.drop_count.fetch_add(self.buf_entries, Relaxed);
            self.sh.drop_bytes.fetch_add(self.buf.len() as u64, Relaxed);
            self.sh.dropped_total.fetch_add(self.buf_entries, Relaxed);
        }
        self.buf.clear();
        self.buf_entries = 0;
        self.scanned = false;
        let retry_at = self.now() + RETRY_MS;
        self.suspended = Some(Suspension { code, retry_at });
        self.publish();
    }

    fn suspend(&mut self, e: &io::Error) {
        self.suspend_with(code_of(e));
    }

    fn on_panic(&mut self) {
        self.suspend_with("JOURNAL_PANIC");
    }

    /// Close: the queued data, a last (short) stats window, the drop report, then flush and
    /// fsync.
    fn shutdown(&mut self) {
        while let Ok(d) = self.data.try_recv() {
            self.on_data(d);
        }
        let now = self.now();
        let end = self
            .windows
            .due_at(self.stats_ms())
            .filter(|due| now < *due)
            .map(|_| now);
        if !self.windows.is_empty() {
            for (source, p) in self.windows.take(end, self.stats_ms()) {
                self.write_own(source, p, now);
            }
        }
        self.report_drops();
        if self.flush().is_ok()
            && let Some(f) = self.active.take()
        {
            let _ = files::sync_file(&f).and(files::sync_dir(&self.dir));
        }
        self.publish();
    }

    /// Publishes what `status()` reports.
    fn publish(&self) {
        let mut s = self.sh.lock_status();
        s.suspended = self.suspended.as_ref().map(|s| s.code);
        s.files = self.files.len() as u64;
        s.bytes = self.files.iter().map(|f| f.bytes).sum();
        s.oldest = self.files.first().map(|f| f.start_ts);
        s.newest = self.files.iter().map(|f| f.end_ts).max();
    }
}
