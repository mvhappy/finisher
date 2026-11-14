//! The writer's file side (plan D, Decisions 16–18): the scan at open (with the tail repair),
//! the active file (made only by a write, a 0-byte newest file reused), rotation, the age
//! and size caps, suspension and close.

use std::io::{self, ErrorKind};
use std::sync::atomic::Ordering::Relaxed;

use crate::files::{self, DAY_MS, FileMeta};
use crate::writer::{RETRY_MS, Suspension, Writer};

pub(crate) fn code_of(e: &io::Error) -> &'static str {
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
    /// Makes the active file ready for a `len`-byte line at `ts`: the first write scans the
    /// directory and opens a file, reusing an empty newest file of the same UTC day (a
    /// suspended journal's retries make no new files); a full file or a new UTC day rotates.
    /// A clock that went back does not rotate (its day is not later than the file's): the
    /// entries go on into the active file, whose `start_ts` and `end_ts` widen to hold them.
    pub fn prepare(&mut self, ts: u64, len: u64) -> io::Result<()> {
        if !self.scanned {
            self.scan()?;
        }
        // The blobs the file references count towards its size, so `maxMiB` can remove them
        // with it: an open file is never removed.
        if self.active.is_some()
            && let Some(m) = self.files.last()
            && ((m.bytes > 0
                && m.bytes + self.active_blob_bytes + len > self.sh.opts.max_file_bytes)
                || ts / DAY_MS > m.day)
        {
            self.rotate()?;
        }
        if self.active.is_none() {
            self.open_active(ts)?;
        }
        Ok(())
    }

    fn open_active(&mut self, ts: u64) -> io::Result<()> {
        files::ensure_dir(&self.dir)?;
        self.active_blob_bytes = 0;
        let day = ts / DAY_MS;
        if let Some(m) = self.files.last()
            && m.bytes == 0
        {
            let path = self.dir.join(&m.name);
            if m.day == day {
                self.active = Some(files::open_append(&path)?);
                self.publish();
                return Ok(());
            }
            files::remove(&path)?;
            self.files.pop();
        }
        let id = self.next_id(ts);
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
        Ok(())
    }

    pub fn rotate(&mut self) -> io::Result<()> {
        self.flush_buf()?;
        if let Some(f) = self.active.take() {
            files::sync_file(&f)?;
        }
        let now = self.now();
        self.apply_caps(now)
    }

    /// Lists the existing files, repairs the newest one's tail and applies the caps. Creates
    /// nothing: a missing directory is an empty journal.
    pub fn scan(&mut self) -> io::Result<()> {
        self.active = None;
        self.files.clear();
        let list = files::list(&self.dir)?;
        let newest = list.len().checked_sub(1);
        for (i, (name, id, len)) in list.into_iter().enumerate() {
            let path = self.dir.join(&name);
            let len = if Some(i) == newest {
                let kept = files::repair_tail(&path, len)?;
                if kept < len {
                    self.sh.faults.note_repair();
                }
                kept
            } else {
                len
            };
            let start_ts = id.timestamp_ms();
            let (end_ts, last) = files::last_entry(&path, len).unwrap_or((start_ts, None));
            self.last_id = self.last_id.max(id).max(last.unwrap_or(id));
            self.files.push(FileMeta {
                name,
                id,
                day: start_ts / DAY_MS,
                start_ts,
                end_ts,
                bytes: len,
            });
        }
        files::remove_temps(&self.dir);
        self.blobs.load(&self.dir, &self.files)?;
        self.scanned = true;
        self.listed = true;
        let now = self.now();
        self.apply_caps(now)
    }

    /// A journal opened off: lists what is on disk and applies the caps, which only delete
    /// (retention holds while off). Repairs nothing and creates nothing.
    pub fn list_only(&mut self) {
        if let Ok(list) = files::list(&self.dir) {
            self.files = list
                .into_iter()
                .map(|(name, id, len)| {
                    let path = self.dir.join(&name);
                    let start_ts = id.timestamp_ms();
                    FileMeta {
                        end_ts: files::last_ts(&path, len).unwrap_or(start_ts),
                        name,
                        id,
                        day: start_ts / DAY_MS,
                        start_ts,
                        bytes: len,
                    }
                })
                .collect();
            files::remove_temps(&self.dir);
            if self.blobs.load(&self.dir, &self.files).is_ok() {
                self.listed = true;
                let now = self.now();
                let _ = self.apply_caps(now);
            }
        }
        self.publish();
    }

    pub fn suspend_with(&mut self, code: &'static str) {
        if let Some(f) = self.active.take() {
            // Best effort: what was written before the error is kept.
            let _ = f.sync_all();
        }
        let entries = self.lines.len() as u64;
        if entries > 0 {
            self.sh.drop_count.fetch_add(entries, Relaxed);
            self.sh.drop_bytes.fetch_add(self.buf.len() as u64, Relaxed);
            self.sh.dropped_total.fetch_add(entries, Relaxed);
        }
        if let Some(m) = self.files.last_mut() {
            m.bytes = m.bytes.saturating_sub(self.buf.len() as u64);
        }
        self.buf.clear();
        self.lines.clear();
        self.scanned = false;
        let retry_at = self.now() + RETRY_MS;
        self.suspended = Some(Suspension { code, retry_at });
        self.publish();
    }

    pub fn suspend(&mut self, e: &io::Error) {
        self.suspend_with(code_of(e));
    }

    pub fn on_panic(&mut self) {
        self.suspend_with("JOURNAL_PANIC");
    }

    /// Close: the queued data, a last (short) stats window, the drop report, then flush and
    /// fsync.
    pub fn shutdown(&mut self) {
        self.drain_data();
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
        self.report_drops(true);
        if self.flush().is_ok()
            && let Some(f) = self.active.take()
        {
            let _ = files::sync_file(&f).and(files::sync_dir(&self.dir));
        }
        self.publish();
    }

    /// Publishes what `status()` reports.
    pub fn publish(&self) {
        let mut s = self.sh.lock_status();
        s.suspended = self.suspended.as_ref().map(|s| s.code);
        s.files = self.files.len() as u64;
        s.bytes = self.files.iter().map(|f| f.bytes).sum();
        s.oldest = self.files.iter().map(|f| f.start_ts).min();
        s.newest = self.files.iter().map(|f| f.end_ts).max();
        s.blobs = self.blobs.count();
        s.blob_bytes = self.blobs.total_bytes();
        s.active = self
            .active
            .as_ref()
            .and_then(|_| self.files.last())
            .map(|f| f.name.clone());
    }
}
