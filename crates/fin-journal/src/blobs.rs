//! Blobs (plan D, Decisions 17 and 20): large content as `events/blobs/<hex>`, addressed by
//! the keyed BLAKE3 of its bytes. The caller hashes and queues the bytes
//! ([`Journal::attach_blob`]); the writer holds them until an entry that references them is
//! written with training, writes the blob (temp-then-rename, fsynced) before that line, and
//! counts the references per file. A blob whose last reference goes (retention, `maxMiB`,
//! purge, a withdrawal that strips the line) is deleted at the next rotation, cap pass or
//! purge; a blob whose entry never got written is never written. A crash can leave an
//! unreferenced blob, never a dangling reference: the next scan collects the orphan.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Acquire;
use std::time::{Duration, Instant};

use fin_core::events::names::MEDIA_TYPES;
use fin_core::events::{BlobRef, Name};

use crate::control::Data;
use crate::files::{self, FileMeta};
use crate::{Journal, encode, key};

/// The largest blob in this phase (4 MiB).
pub const MAX_BLOB_BYTES: usize = 4 << 20;
/// How long the writer keeps a blob no entry has referenced yet.
const PENDING_FOR: Duration = Duration::from_secs(10);
/// What the writer keeps of such blobs at most.
const PENDING_BYTES: usize = 8 << 20;

pub(crate) const BLOB_DIR: &str = "blobs";

/// A blob on its way to the writer.
#[derive(Debug)]
pub(crate) struct BlobData {
    pub hex: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Debug, Default)]
pub(crate) struct Blobs {
    /// References per blob, over every file.
    refs: HashMap<String, u32>,
    /// Each file's references (a blob once per referencing line).
    by_file: HashMap<String, Vec<String>>,
    /// The blobs on disk and their sizes.
    sizes: HashMap<String, u64>,
    pending: VecDeque<(BlobData, Instant)>,
    pending_bytes: usize,
    /// Blobs of the line being written: a rotation's collection must not take them before
    /// the line counts its references.
    pinned: Vec<String>,
}

pub(crate) fn blob_dir(events: &Path) -> PathBuf {
    events.join(BLOB_DIR)
}

fn is_blob_name(n: &str) -> bool {
    n.len() == 64 && n.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Blobs {
    pub fn clear(&mut self) {
        *self = Blobs::default();
    }

    /// Lists `events/blobs/` (removing temp files a crash left) and, when it holds any blob,
    /// counts the references of `files`' lines. Writes nothing but those removals.
    pub fn load(&mut self, events: &Path, list: &[FileMeta]) -> io::Result<()> {
        self.refs.clear();
        self.by_file.clear();
        self.sizes.clear();
        let dir = blob_dir(events);
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        for e in rd {
            let e = e?;
            let Some(name) = e.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') && name.ends_with(".tmp") {
                let _ = fs::remove_file(e.path());
            } else if is_blob_name(&name) {
                self.sizes.insert(name, e.metadata()?.len());
            }
        }
        if self.sizes.is_empty() {
            return Ok(());
        }
        for f in list {
            let Ok(text) = fs::read(events.join(&f.name)) else {
                continue;
            };
            let hexes = blobs_in(&text);
            self.add(&f.name, &hexes);
        }
        Ok(())
    }

    pub fn count(&self) -> u64 {
        self.sizes.len() as u64
    }

    pub fn size_of(&self, hex: &str) -> u64 {
        self.sizes.get(hex).copied().unwrap_or(0)
    }

    pub fn total_bytes(&self) -> u64 {
        self.sizes.values().sum()
    }

    /// Holds a blob until an entry references it.
    pub fn offer(&mut self, b: BlobData) {
        self.evict(Instant::now());
        if self.sizes.contains_key(&b.hex) || self.pending.iter().any(|(p, _)| p.hex == b.hex) {
            return;
        }
        self.pending_bytes += b.bytes.len();
        self.pending.push_back((b, Instant::now()));
        while self.pending_bytes > PENDING_BYTES
            && let Some((old, _)) = self.pending.pop_front()
        {
            self.pending_bytes -= old.bytes.len();
        }
    }

    /// Drops pending blobs no entry referenced in time.
    pub fn evict(&mut self, now: Instant) {
        while let Some((b, at)) = self.pending.front()
            && now.duration_since(*at) > PENDING_FOR
        {
            self.pending_bytes -= b.bytes.len();
            self.pending.pop_front();
        }
    }

    /// Makes sure every blob of `hexes` is on disk, writing the pending ones; false when one
    /// is neither (its bytes were dropped): the entry must go without its content.
    pub fn ensure(&mut self, events: &Path, hexes: &[String]) -> io::Result<bool> {
        for hex in hexes {
            if self.sizes.contains_key(hex) {
                continue;
            }
            let Some(i) = self.pending.iter().position(|(p, _)| &p.hex == hex) else {
                return Ok(false);
            };
            let dir = blob_dir(events);
            files::ensure_dir(&dir)?;
            let (b, _) = &self.pending[i];
            key::write_private(&dir, hex, &b.bytes)?;
            let (b, _) = self.pending.remove(i).expect("index found above");
            self.pending_bytes -= b.bytes.len();
            self.sizes.insert(b.hex, b.bytes.len() as u64);
        }
        Ok(true)
    }

    pub fn add(&mut self, file: &str, hexes: &[String]) {
        if hexes.is_empty() {
            return;
        }
        for h in hexes {
            *self.refs.entry(h.clone()).or_default() += 1;
        }
        self.by_file
            .entry(file.to_owned())
            .or_default()
            .extend_from_slice(hexes);
    }

    /// Lowers the counts of `hexes` held by `file` (a line rewritten without them).
    pub fn unref(&mut self, file: &str, hexes: &[String]) {
        let Some(list) = self.by_file.get_mut(file) else {
            return;
        };
        let mut gone = Vec::new();
        for h in hexes {
            if let Some(i) = list.iter().position(|x| x == h) {
                list.swap_remove(i);
                gone.push(h);
            }
        }
        for h in gone {
            self.lower(h);
        }
    }

    fn lower(&mut self, h: &str) {
        if let Some(n) = self.refs.get_mut(h) {
            *n -= 1;
            if *n == 0 {
                self.refs.remove(h);
            }
        }
    }

    /// Every reference of a deleted file.
    pub fn drop_file(&mut self, file: &str) {
        for h in self.by_file.remove(file).unwrap_or_default() {
            self.lower(&h);
        }
    }

    /// A file rewritten: its references are now `hexes`.
    pub fn set_file(&mut self, file: &str, hexes: &[String]) {
        self.drop_file(file);
        self.add(file, hexes);
    }

    /// The blob bytes deleting `files` would free (their last references).
    pub fn freed_by<'a>(&self, files: impl Iterator<Item = &'a str>) -> u64 {
        let mut gone: HashMap<&str, u32> = HashMap::new();
        let mut freed = 0;
        for f in files {
            for h in self.by_file.get(f).into_iter().flatten() {
                let n = gone.entry(h.as_str()).or_default();
                *n += 1;
                if self.refs.get(h).copied() == Some(*n) {
                    freed += self.sizes.get(h).copied().unwrap_or(0);
                }
            }
        }
        freed
    }

    /// The bytes of blobs on disk that nothing references (orphans).
    pub fn orphan_bytes(&self) -> u64 {
        self.sizes
            .iter()
            .filter(|(h, _)| !self.refs.contains_key(*h))
            .map(|(_, n)| n)
            .sum()
    }

    pub fn pin(&mut self, hexes: &[String]) {
        self.pinned = hexes.to_vec();
    }

    pub fn unpin(&mut self) {
        self.pinned.clear();
    }

    /// Deletes every blob on disk nothing references; returns how many went.
    pub fn gc(&mut self, events: &Path) -> io::Result<u64> {
        let dead: Vec<String> = self
            .sizes
            .keys()
            .filter(|h| !self.refs.contains_key(*h) && !self.pinned.contains(h))
            .cloned()
            .collect();
        if dead.is_empty() {
            return Ok(0);
        }
        let dir = blob_dir(events);
        let mut n = 0;
        for h in dead {
            key::remove(&dir, &h)?;
            self.sizes.remove(&h);
            n += 1;
        }
        files::sync_dir(&dir)?;
        Ok(n)
    }
}

/// The blob references in a file's lines (only lines that can hold one are parsed).
pub(crate) fn blobs_in(text: &[u8]) -> Vec<String> {
    text.split(|b| *b == b'\n')
        .filter(|l| contains(l, b"\"kb:"))
        .flat_map(encode::line_blobs)
        .map(|b| b.hash.hex().to_owned())
        .collect()
}

pub(crate) fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

impl Journal {
    /// Hashes `bytes` with the install key and queues them for the writer, which writes the
    /// blob before the first line that references it (Decision 20). `None` unless the gate
    /// has training, for a blob over [`MAX_BLOB_BYTES`], or for a media type outside
    /// `MEDIA_TYPES`. The bytes count against the channel's byte bound: when they are
    /// dropped, the entry that references them is written without its content.
    pub fn attach_blob(&self, bytes: Arc<[u8]>, media: Name) -> Option<BlobRef> {
        let h = self.h.as_ref()?;
        let word = h.sh.word.load(Acquire);
        if !crate::control::gate_of(word).has_training()
            || bytes.len() > MAX_BLOB_BYTES
            || MEDIA_TYPES.get(media.as_str()).is_none()
        {
            return None;
        }
        let key = h.sh.key()?;
        let hash = key.blob_hash(&bytes);
        let r = BlobRef {
            hash,
            bytes: bytes.len() as u64,
            media_type: media,
        };
        let len = bytes.len();
        let hex = r.hash.hex().to_owned();
        h.send(Data::Blob(Box::new(BlobData { hex, bytes })), len);
        Some(r)
    }
}
