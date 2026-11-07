//! Purge and the caps (plan D, Decisions 16, 20 and 29), on the writer thread so one owner
//! touches the files. A purge closes the active file, rewrites each affected closed file
//! without the matching entries (temp-then-rename, fsynced: a kill leaves the old file or the
//! new one, never a mix), deletes files left empty, lowers the blob counts of what it
//! removed and deletes the blobs left at zero. Retention and `maxMiB` delete whole files,
//! oldest first, counting blobs, through the same blob bookkeeping.

use std::fs;
use std::io::{self, ErrorKind};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc;
use std::time::Duration;

use fin_core::events::{Envelope, ProjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::blobs::{self, blob_dir};
use crate::control::Control;
use crate::files::{self, FileMeta};
use crate::key::{self, KEY_FILE};
use crate::state::STATE_FILE;
use crate::writer::{CAPS_EVERY_MS, Writer};
use crate::{Journal, JournalError};

/// What to purge (`fin diagnostics purge`, `diagnostics.purge`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PurgeSelector {
    /// Everything: every file, every blob, the key and `state.json`.
    All,
    /// The entries of one project (`ctx.project`; a project id, lower-cased).
    Project(String),
    /// The entries with `ts` before this (ms since the epoch).
    Before(u64),
}

/// What a purge removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    /// Entries.
    pub removed: u64,
    /// Files deleted or rewritten.
    pub files: u64,
    pub blobs: u64,
}

fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split_inclusive(|b| *b == b'\n')
        .filter(|l| l.ends_with(b"\n"))
}

impl Writer {
    /// Retention and `maxMiB` (whole closed files, oldest first; blobs count, and go with
    /// their last reference), then the blobs nothing references.
    pub fn apply_caps(&mut self, now: u64) -> io::Result<()> {
        self.next_caps = now + CAPS_EVERY_MS;
        let max_age = self.sh.caps_age.load(Relaxed);
        let max_total = self.sh.caps_total.load(Relaxed);
        let closed =
            self.files.len() - usize::from(self.active.is_some() && !self.files.is_empty());
        let cutoff = now.saturating_sub(max_age);
        let mut n = self.files[..closed]
            .iter()
            .take_while(|f| f.end_ts < cutoff)
            .count();
        let files_total: u64 = self.files.iter().map(|f| f.bytes).sum();
        let blobs_live = self.blobs.total_bytes() - self.blobs.orphan_bytes();
        let freed = |w: &Writer, n: usize| {
            w.files[..n].iter().map(|f| f.bytes).sum::<u64>()
                + w.blobs
                    .freed_by(w.files[..n].iter().map(|f| f.name.as_str()))
        };
        while n < closed && files_total + blobs_live - freed(self, n) > max_total {
            n += 1;
        }
        let mut result = Ok(());
        let mut removed = 0;
        for f in &self.files[..n] {
            match files::remove(&self.dir.join(&f.name)) {
                Ok(()) => removed += 1,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        for f in self.files.drain(..removed) {
            self.blobs.drop_file(&f.name);
        }
        if removed > 0 {
            result = result.and(files::sync_dir(&self.dir));
        }
        result = result.and(self.blobs.gc(&self.dir).map(|_| ()));
        self.publish();
        result
    }

    /// A purge, from its control message.
    pub fn purge(&mut self, sel: &PurgeSelector) -> io::Result<PurgeReport> {
        if !self.scanned && !self.listed {
            self.list_only();
        }
        self.flush_buf()?;
        if let Some(f) = self.active.take() {
            files::sync_file(&f)?;
        }
        let mut r = match sel {
            PurgeSelector::All => self.purge_all()?,
            PurgeSelector::Project(id) => {
                let needle = format!("\"project\":\"{id}\"");
                self.rewrite_each(
                    |f| (false, Some(needle.clone()), f.bytes > 0),
                    |e| {
                        e.ctx
                            .as_ref()
                            .and_then(|c| c.project.as_ref())
                            .is_some_and(|p| p.as_str() == id)
                    },
                )?
            }
            PurgeSelector::Before(ts) => {
                let ts = *ts;
                self.rewrite_each(|f| (f.end_ts < ts, None, f.start_ts < ts), |e| e.ts < ts)?
            }
        };
        r.blobs += self.blobs.gc(&self.dir)?;
        self.publish();
        Ok(r)
    }

    fn purge_all(&mut self) -> io::Result<PurgeReport> {
        let mut r = PurgeReport::default();
        for f in std::mem::take(&mut self.files) {
            let path = self.dir.join(&f.name);
            r.removed += fs::read(&path).map_or(0, |t| lines(&t).count() as u64);
            key::remove(&self.dir, &f.name)?;
            r.files += 1;
        }
        let bdir = blob_dir(&self.dir);
        match fs::read_dir(&bdir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    files::remove(&e.path())?;
                    r.blobs += 1;
                }
                fs::remove_dir(&bdir)?;
            }
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(e) => return Err(e),
        }
        self.blobs.clear();
        key::remove(&self.dir, KEY_FILE)?;
        // The caller swapped in a new key before this message; a flush in between may have
        // written it, and the file is gone now: it is written again before the next line.
        if let Some(k) = self.sh.key() {
            k.forget_persisted();
        }
        key::remove(&self.dir, STATE_FILE)?;
        self.state_dirty = true;
        self.state_other.clear();
        if self.dir.exists() {
            files::sync_dir(&self.dir)?;
            // Empty now: the next allowed write makes it again.
            let _ = fs::remove_dir(&self.dir);
        }
        Ok(r)
    }

    /// For each file, `plan` says (delete whole, a byte string a matching line must contain,
    /// rewrite at all); a rewrite drops the lines `hit` matches.
    fn rewrite_each(
        &mut self,
        plan: impl Fn(&FileMeta) -> (bool, Option<String>, bool),
        hit: impl Fn(&Envelope) -> bool,
    ) -> io::Result<PurgeReport> {
        let mut r = PurgeReport::default();
        let mut i = 0;
        while i < self.files.len() {
            let f = self.files[i].clone();
            let path = self.dir.join(&f.name);
            let (whole, needle, rewrite) = plan(&f);
            if !whole && !rewrite {
                i += 1;
                continue;
            }
            let text = match fs::read(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    self.blobs.drop_file(&f.name);
                    self.files.remove(i);
                    continue;
                }
                Err(e) => return Err(e),
            };
            if whole {
                r.removed += lines(&text).count() as u64;
                key::remove(&self.dir, &f.name)?;
                files::sync_dir(&self.dir)?;
                self.blobs.drop_file(&f.name);
                self.files.remove(i);
                r.files += 1;
                continue;
            }
            if needle
                .as_ref()
                .is_some_and(|n| !blobs::contains(&text, n.as_bytes()))
            {
                i += 1;
                continue;
            }
            let mut kept = Vec::with_capacity(text.len());
            let mut removed = 0;
            for l in lines(&text) {
                let candidate = needle
                    .as_ref()
                    .is_none_or(|n| blobs::contains(l, n.as_bytes()));
                let e = candidate
                    .then(|| serde_json::from_slice::<Envelope>(l).ok())
                    .flatten();
                if e.as_ref().is_some_and(&hit) {
                    removed += 1;
                    continue;
                }
                kept.extend_from_slice(l);
            }
            if removed == 0 {
                i += 1;
                continue;
            }
            r.removed += removed;
            r.files += 1;
            if kept.is_empty() {
                key::remove(&self.dir, &f.name)?;
                files::sync_dir(&self.dir)?;
                self.blobs.drop_file(&f.name);
                self.files.remove(i);
            } else {
                self.sh.faults.purge_write()?;
                let tmp = key::write_temp(&self.dir, &f.name, &kept)?;
                self.sh.faults.before_purge_rename();
                files::rename(&tmp, &path)?;
                files::sync_dir(&self.dir)?;
                self.blobs.set_file(&f.name, &blobs::blobs_in(&kept));
                let m = &mut self.files[i];
                m.bytes = kept.len() as u64;
                m.end_ts = files::last_ts(&path, m.bytes).unwrap_or(m.start_ts);
                i += 1;
            }
            self.sh.faults.in_purge();
        }
        Ok(r)
    }
}

impl Journal {
    /// Purges on the writer thread and waits for its report (at most 60 s). Works while off:
    /// a purge only deletes. `all` also deletes the key (old and new hashes are then
    /// unlinkable) and `state.json`. Blocks: call it off the JS thread.
    pub fn purge(&self, sel: PurgeSelector) -> Result<PurgeReport, JournalError> {
        let Some(h) = &self.h else {
            return Ok(PurgeReport::default());
        };
        let sel = match sel {
            PurgeSelector::Project(p) => {
                let p = p.to_ascii_lowercase();
                if ProjectId::parse(&p).is_none() {
                    return Err(JournalError::new(
                        "INVALID_ARGS",
                        format!("{p:?} is not a project id"),
                    ));
                }
                PurgeSelector::Project(p)
            }
            s => s,
        };
        if sel == PurgeSelector::All {
            // Entries recorded from now on hash with a new key; the queued ones are purged.
            h.sh.reset_key(self.gate().is_some());
        }
        let (ack, rx) = mpsc::sync_channel(1);
        if !h.control(Control::Purge { sel, ack }) {
            return Err(JournalError::new(
                "JOURNAL_UNAVAILABLE",
                "the journal is closed",
            ));
        }
        rx.recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| {
                Err(JournalError::new(
                    "JOURNAL_TIMEOUT",
                    "the journal writer did not answer",
                ))
            })
    }
}
