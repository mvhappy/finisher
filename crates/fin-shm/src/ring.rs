//! Ring writers and readers (spec §3.4).
//!
//! **Slot protocol** (a sequence lock per slot, one writer per ring): the writer stores an odd
//! `seq`, then `len` and the payload, then the next even `seq` with release ordering. A reader
//! reads `seq`, then (after an acquire fence) `len` and the payload, then `seq` again; a slot
//! whose two reads differ, or whose `seq` is odd, is [`SlotState::Busy`] and its copy is
//! discarded. A `len` over the slot size with a stable `seq` is a protocol error.
//!
//! **Mapping:** a [`RingWriter`] maps its own file. A [`RingReader`] opened with
//! [`RingReader::open_untrusted`] never maps: it reads with bounded `pread` into the caller's
//! buffer, re-checks the file's size and links at every read, and a short read is
//! `truncated`, never a `SIGBUS`. [`RingReader::open_sealed`] maps read-only and is only for a
//! ring whose writer has exited.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering, fence};

use fin_core::CoreError;
use memmap2::{Mmap, MmapMut};

use crate::checks::{Facts, Fail, Reason};
use crate::hooks;
use crate::layout::{HEADER_BYTES, RingLayout, RingSpec, decode, encode_header};
use crate::open::{Mode, create_new, facts, open_checked, read_exact_at, to_error};

/// What [`RingReader::read_slot`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Never written (`seq == 0`); the buffer is empty.
    Empty,
    /// A finished slot: the buffer holds its `len` payload bytes, written under `seq`.
    Ready { seq: u64, len: u64 },
    /// Being written (or abandoned mid-write by a writer that exited), or rewritten while it
    /// was copied; the buffer is empty. Read it again later.
    Busy,
}

fn usize_of(v: u64) -> Result<usize, Fail> {
    usize::try_from(v)
        .map_err(|_| Fail::new(Reason::OverCap, "the ring is too large for this platform"))
}

fn check_layout(spec: &RingSpec, cap: u64) -> Result<RingLayout, Fail> {
    let l = spec.checked_layout()?;
    if l.file_bytes > cap {
        return Err(Fail::new(
            Reason::OverCap,
            format!("a ring of {} bytes is over the cap of {cap}", l.file_bytes),
        ));
    }
    usize_of(l.file_bytes)?;
    Ok(l)
}

/// The writer of a ring: maps its own file (`out/` of the writing party).
pub struct RingWriter {
    path: PathBuf,
    layout: RingLayout,
    map: MmapMut,
    _file: File,
}

impl RingWriter {
    /// Creates the ring at `path` (it must not exist; no segment may be a link), at its final
    /// size, writes the header and maps it. `cap` bounds the file size.
    pub fn create(path: &Path, spec: RingSpec, cap: u64) -> Result<RingWriter, CoreError> {
        let layout = check_layout(&spec, cap).map_err(|f| to_error(path, f))?;
        let opened = create_new(path)?;
        opened
            .file
            .set_len(layout.file_bytes)
            .map_err(|e| to_error(path, Fail::io(e)))?;
        let mut w = RingWriter::map(path, opened.file, layout)?;
        w.map[..HEADER_BYTES as usize].copy_from_slice(&encode_header(&spec));
        Ok(w)
    }

    /// Opens a ring file 5C created at its final size (zero-filled: the header is written;
    /// already a ring of this `spec`: kept, sequences continue). Every open check applies, and
    /// the size must be the spec's.
    pub fn open_sized(path: &Path, spec: RingSpec, cap: u64) -> Result<RingWriter, CoreError> {
        let layout = check_layout(&spec, cap).map_err(|f| to_error(path, f))?;
        let opened = open_checked(path, cap, Mode::ReadWrite)?;
        if opened.facts.size != layout.file_bytes {
            return Err(to_error(
                path,
                size_mismatch(opened.facts.size, layout.file_bytes),
            ));
        }
        let mut w = RingWriter::map(path, opened.file, layout)?;
        let header = &mut w.map[..HEADER_BYTES as usize];
        if header.iter().all(|b| *b == 0) {
            header.copy_from_slice(&encode_header(&spec));
        } else if decode(header).map(|l| l.spec).ok() != Some(spec) {
            return Err(to_error(
                path,
                Fail::new(Reason::Header, "the file holds another ring's header"),
            ));
        }
        Ok(w)
    }

    fn map(path: &Path, file: File, layout: RingLayout) -> Result<RingWriter, CoreError> {
        hooks::count_map();
        // SAFETY: the writer's own file in its own `out/`, created or checked at its final
        // size; no more-trusted party truncates it while it is mapped (spec §3.4).
        let map = unsafe { MmapMut::map_mut(&file) }.map_err(|e| to_error(path, Fail::io(e)))?;
        if map.len() as u64 != layout.file_bytes {
            return Err(to_error(
                path,
                size_mismatch(map.len() as u64, layout.file_bytes),
            ));
        }
        Ok(RingWriter {
            path: path.to_path_buf(),
            layout,
            map,
            _file: file,
        })
    }

    pub fn spec(&self) -> RingSpec {
        self.layout.spec
    }

    pub fn layout(&self) -> RingLayout {
        self.layout
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `payload` into slot `i`; answers the slot's new (even) `seq`.
    pub fn write_slot(&mut self, i: u32, payload: &[u8]) -> Result<u64, CoreError> {
        self.write_slot_with(i, payload.len() as u64, |buf| buf.copy_from_slice(payload))
    }

    /// Writes slot `i` in place: `fill` gets the slot's first `len` bytes (a decoder writes a
    /// frame straight into the ring). Answers the slot's new (even) `seq`.
    pub fn write_slot_with(
        &mut self,
        i: u32,
        len: u64,
        fill: impl FnOnce(&mut [u8]),
    ) -> Result<u64, CoreError> {
        let l = self.layout;
        l.check_index(i).map_err(|f| to_error(&self.path, f))?;
        if len > l.spec.slot_bytes {
            let msg = format!(
                "a payload of {len} bytes is over the slot size of {}",
                l.spec.slot_bytes
            );
            return Err(to_error(&self.path, Fail::new(Reason::Protocol, msg)));
        }
        let base = self.map.as_mut_ptr();
        // In range and 8-aligned: the map is page-aligned and the table entries are 16 bytes
        // from offset 32; the payload range lies inside the file (`checked_layout`).
        let seq_off = l.seq_offset(i) as usize;
        let slot_off = l.slot_offset(i) as usize;
        // SAFETY: `base + seq_off` and `base + seq_off + 8` are aligned u64s inside the map;
        // the payload range is inside the map and does not overlap the table. Everything is
        // derived from one raw pointer, and `&mut self` makes this the only writer here.
        unsafe {
            let seq = AtomicU64::from_ptr(base.add(seq_off).cast::<u64>());
            let len_at = AtomicU64::from_ptr(base.add(seq_off + 8).cast::<u64>());
            let cur = u64::from_le(seq.load(Ordering::Relaxed));
            let odd = cur | 1;
            let Some(next) = odd.checked_add(1) else {
                return Err(to_error(
                    &self.path,
                    Fail::new(Reason::Protocol, "the slot's sequence is exhausted"),
                ));
            };
            seq.store(odd.to_le(), Ordering::Relaxed);
            fence(Ordering::Release);
            len_at.store(len.to_le(), Ordering::Relaxed);
            fill(std::slice::from_raw_parts_mut(
                base.add(slot_off),
                len as usize,
            ));
            seq.store(next.to_le(), Ordering::Release);
            Ok(next)
        }
    }

    /// Flushes the mapping to the file (`msync`).
    pub fn flush(&self) -> Result<(), CoreError> {
        self.map
            .flush()
            .map_err(|e| to_error(&self.path, Fail::io(e)))
    }
}

impl std::fmt::Debug for RingWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RingWriter")
            .field("path", &self.path)
            .field("layout", &self.layout)
            .finish()
    }
}

impl std::fmt::Debug for RingReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RingReader")
            .field("path", &self.path)
            .field("layout", &self.layout)
            .field("mapped", &self.map.is_some())
            .finish()
    }
}

fn size_mismatch(size: u64, declared: u64) -> Fail {
    Fail::new(
        Reason::SizeMismatch,
        format!("the file has {size} bytes; its ring header declares {declared}"),
    )
}

/// A reader of another party's ring.
pub struct RingReader {
    path: PathBuf,
    layout: RingLayout,
    file: File,
    map: Option<Mmap>,
}

impl RingReader {
    /// Opens a ring a less-trusted process may still be writing: every open check, the header,
    /// a size equal to the header's and at most `cap`. Never maps; reads are bounded `pread`s.
    pub fn open_untrusted(path: &Path, cap: u64) -> Result<RingReader, CoreError> {
        let (file, layout) = RingReader::open_checked(path, cap)?;
        Ok(RingReader {
            path: path.to_path_buf(),
            layout,
            file,
            map: None,
        })
    }

    /// Opens a ring whose writer has exited (or an `in/` ring written before the reader
    /// started, spec §3.4): the same checks, then a read-only mapping.
    pub fn open_sealed(path: &Path, cap: u64) -> Result<RingReader, CoreError> {
        let (file, layout) = RingReader::open_checked(path, cap)?;
        hooks::count_map();
        // SAFETY: the caller promises the writer can no longer write or truncate the file;
        // the size was checked against the header just now.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| to_error(path, Fail::io(e)))?;
        if map.len() as u64 != layout.file_bytes {
            return Err(to_error(
                path,
                size_mismatch(map.len() as u64, layout.file_bytes),
            ));
        }
        Ok(RingReader {
            path: path.to_path_buf(),
            layout,
            file,
            map: Some(map),
        })
    }

    fn open_checked(path: &Path, cap: u64) -> Result<(File, RingLayout), CoreError> {
        let opened = open_checked(path, cap, Mode::Read)?;
        let fail = |f| to_error(path, f);
        if opened.facts.size < HEADER_BYTES {
            let msg = format!(
                "the file has {} bytes, too short for a ring header",
                opened.facts.size
            );
            return Err(fail(Fail::new(Reason::Header, msg)));
        }
        let mut h = [0u8; HEADER_BYTES as usize];
        read_exact_at(&opened.file, &mut h, 0).map_err(fail)?;
        let layout = decode(&h).map_err(fail)?;
        if layout.file_bytes > cap {
            let msg = format!(
                "the ring declares {} bytes, over the cap of {cap}",
                layout.file_bytes
            );
            return Err(fail(Fail::new(Reason::OverCap, msg)));
        }
        if opened.facts.size != layout.file_bytes {
            return Err(fail(size_mismatch(opened.facts.size, layout.file_bytes)));
        }
        usize_of(layout.file_bytes).map_err(fail)?;
        Ok((opened.file, layout))
    }

    pub fn spec(&self) -> RingSpec {
        self.layout.spec
    }

    pub fn layout(&self) -> RingLayout {
        self.layout
    }

    /// Whether this reader maps the file (only after [`RingReader::open_sealed`]).
    pub fn is_mapped(&self) -> bool {
        self.map.is_some()
    }

    /// Copies slot `i` into `buf` (resized to the payload) if it is finished and unchanged
    /// while copied. An out-of-range index or a `len` over the slot size is `protocol`; a
    /// truncated file `truncated`; a size or link change since the open `changed` or
    /// `hardLinked`.
    pub fn read_slot(&mut self, i: u32, buf: &mut Vec<u8>) -> Result<SlotState, CoreError> {
        buf.clear();
        let r = match &self.map {
            Some(map) => read_mapped(&self.layout, map, i, buf),
            None => read_pread(&self.layout, &self.file, i, buf),
        };
        r.inspect_err(|_| buf.clear())
            .map_err(|f| to_error(&self.path, f))
    }
}

fn u64_at(file: &File, offset: u64) -> Result<u64, Fail> {
    let mut b = [0u8; 8];
    read_exact_at(file, &mut b, offset)?;
    Ok(u64::from_le_bytes(b))
}

fn over_slot(len: u64, l: &RingLayout) -> Fail {
    Fail::new(
        Reason::Protocol,
        format!(
            "slot length {len} is over the slot size of {}",
            l.spec.slot_bytes
        ),
    )
}

/// The size and link checks at every read of an untrusted ring.
fn recheck(l: &RingLayout, now: Facts) -> Result<(), Fail> {
    if now.nlink != 1 {
        return Err(Fail::new(
            Reason::HardLinked,
            format!("the ring now has {} links", now.nlink),
        ));
    }
    if now.size != l.file_bytes {
        let msg = format!(
            "the ring changed size to {} bytes (declared {})",
            now.size, l.file_bytes
        );
        return Err(Fail::new(Reason::Changed, msg));
    }
    Ok(())
}

/// The reads come first, so a truncation under them is a short read (`truncated`); the size
/// and link re-check follows, before anything is answered.
fn read_pread(l: &RingLayout, file: &File, i: u32, buf: &mut Vec<u8>) -> Result<SlotState, Fail> {
    l.check_index(i)?;
    let state = copy_slot(l, file, i, buf)?;
    recheck(l, facts(file)?)?;
    Ok(state)
}

fn copy_slot(l: &RingLayout, file: &File, i: u32, buf: &mut Vec<u8>) -> Result<SlotState, Fail> {
    let at = l.seq_offset(i);
    let s1 = u64_at(file, at)?;
    if s1 == 0 {
        return Ok(SlotState::Empty);
    }
    if s1 & 1 == 1 {
        return Ok(SlotState::Busy);
    }
    fence(Ordering::Acquire);
    let len = u64_at(file, at + 8)?;
    if len > l.spec.slot_bytes {
        fence(Ordering::Acquire);
        return if u64_at(file, at)? == s1 {
            Err(over_slot(len, l))
        } else {
            Ok(SlotState::Busy)
        };
    }
    buf.resize(usize_of(len)?, 0);
    read_exact_at(file, buf, l.slot_offset(i))?;
    fence(Ordering::Acquire);
    if u64_at(file, at)? != s1 {
        buf.clear();
        return Ok(SlotState::Busy);
    }
    Ok(SlotState::Ready { seq: s1, len })
}

fn read_mapped(l: &RingLayout, map: &[u8], i: u32, buf: &mut Vec<u8>) -> Result<SlotState, Fail> {
    l.check_index(i)?;
    let at = l.seq_offset(i) as usize;
    let word = |o: usize| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&map[o..o + 8]);
        u64::from_le_bytes(b)
    };
    let seq = word(at);
    if seq == 0 {
        return Ok(SlotState::Empty);
    }
    if seq & 1 == 1 {
        return Ok(SlotState::Busy);
    }
    let len = word(at + 8);
    if len > l.spec.slot_bytes {
        return Err(over_slot(len, l));
    }
    let start = l.slot_offset(i) as usize;
    buf.extend_from_slice(&map[start..start + len as usize]);
    Ok(SlotState::Ready { seq, len })
}
