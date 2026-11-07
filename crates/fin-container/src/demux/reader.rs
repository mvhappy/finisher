//! Box headers read by `fin-container` itself (plan 6A Decision 10): every size is checked
//! against its parent's end (the file's, at the top) before anything is read, every box counts
//! against `max_boxes`, nesting against `max_depth`, and a body is read into memory only when
//! it is within `max_box_bytes`. Leaf bodies are then decoded by `mp4-atom`.

use std::io::{Read, Seek, SeekFrom};

use fin_core::CoreError;
use mp4_atom::Atom;

use crate::Limits;
use crate::error::{atom, corrupt, io, limit};

/// A box's place in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoxHeader {
    pub kind: [u8; 4],
    /// Offset of the header.
    pub start: u64,
    /// Offset of the body.
    pub body: u64,
    /// One past the last byte.
    pub end: u64,
}

impl BoxHeader {
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }

    pub fn body_len(&self) -> u64 {
        self.end - self.body
    }
}

pub(crate) struct Reader<'a, R: Read + Seek> {
    r: &'a mut R,
    pub len: u64,
    pub limits: Limits,
    boxes: u32,
}

impl<'a, R: Read + Seek> Reader<'a, R> {
    pub fn new(r: &'a mut R, limits: Limits) -> Result<Self, CoreError> {
        let len = r.seek(SeekFrom::End(0)).map_err(|e| io(&e))?;
        Ok(Reader {
            r,
            len,
            limits,
            boxes: 0,
        })
    }

    /// Reads exactly `buf.len()` bytes at `at`.
    pub fn read_at(&mut self, at: u64, buf: &mut [u8]) -> Result<(), CoreError> {
        self.r.seek(SeekFrom::Start(at)).map_err(|e| io(&e))?;
        self.r.read_exact(buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => corrupt("the file ends inside a read", at, "file"),
            _ => io(&e),
        })
    }

    /// The header of the box at `at`, which must end by `parent_end`.
    pub fn header(&mut self, at: u64, parent_end: u64, depth: u8) -> Result<BoxHeader, CoreError> {
        self.boxes += 1;
        if self.boxes > self.limits.max_boxes {
            return Err(limit(
                "maxBoxes",
                self.boxes as u64,
                self.limits.max_boxes as u64,
            ));
        }
        if depth > self.limits.max_depth {
            return Err(limit(
                "maxDepth",
                depth as u64,
                self.limits.max_depth as u64,
            ));
        }
        if parent_end.saturating_sub(at) < 8 {
            return Err(corrupt("a box header is cut short", at, "header"));
        }
        let mut h = [0u8; 8];
        self.read_at(at, &mut h)?;
        let kind: [u8; 4] = h[4..8].try_into().expect("4 bytes");
        let name = String::from_utf8_lossy(&kind).into_owned();
        let (size, head) = match u32::from_be_bytes(h[..4].try_into().expect("4 bytes")) {
            0 => (parent_end - at, 8),
            1 => {
                if parent_end - at < 16 {
                    return Err(corrupt("a 64-bit box size is cut short", at, &name));
                }
                let mut l = [0u8; 8];
                self.read_at(at + 8, &mut l)?;
                (u64::from_be_bytes(l), 16)
            }
            n => (n as u64, 8),
        };
        if size < head {
            return Err(corrupt(
                format!("the {name} box declares {size} bytes, less than its header"),
                at,
                &name,
            ));
        }
        if size > parent_end - at {
            return Err(corrupt(
                format!(
                    "the {name} box declares {size} bytes but only {} are left",
                    parent_end - at
                ),
                at,
                &name,
            ));
        }
        Ok(BoxHeader {
            kind,
            start: at,
            body: at + head,
            end: at + size,
        })
    }

    /// The headers of the boxes in `[from, to)`; trailing bytes too short for a header (zero
    /// padding some writers add) are ignored.
    pub fn children(&mut self, from: u64, to: u64, depth: u8) -> Result<Vec<BoxHeader>, CoreError> {
        let mut out = Vec::new();
        let mut at = from;
        while to - at >= 8 {
            let h = self.header(at, to, depth)?;
            at = h.end;
            out.push(h);
        }
        Ok(out)
    }

    /// The body of `h`, read whole (`MEDIA_LIMIT` past `max_box_bytes`).
    pub fn body(&mut self, h: &BoxHeader) -> Result<Vec<u8>, CoreError> {
        let len = h.body_len();
        if len > self.limits.max_box_bytes {
            return Err(limit("maxBoxBytes", len, self.limits.max_box_bytes));
        }
        let mut buf = vec![0u8; len as usize];
        self.read_at(h.body, &mut buf)?;
        Ok(buf)
    }
}

/// `body` decoded by `mp4-atom` as a `T` (its size was checked when it was read).
pub(crate) fn decode<T: Atom>(body: &[u8], h: &BoxHeader) -> Result<T, CoreError> {
    let mut buf = body;
    T::decode_body(&mut buf).map_err(|e| atom(e, h.start, &h.name()))
}

/// Checks a declared entry count against the bytes the box has: `count × entry` bytes after
/// `skip` header bytes. Run before `mp4-atom` sees the box, so no declared count is trusted.
pub(crate) fn check_count(
    body: &[u8],
    skip: usize,
    count: u64,
    entry: u64,
    h: &BoxHeader,
) -> Result<(), CoreError> {
    let have = (body.len() as u64).saturating_sub(skip as u64);
    if count.saturating_mul(entry) > have {
        return Err(corrupt(
            format!(
                "the {} box declares {count} entries but has room for {}",
                h.name(),
                have / entry.max(1)
            ),
            h.start,
            &h.name(),
        ));
    }
    Ok(())
}

pub(crate) fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes(s.try_into().expect("4 bytes")))
}

pub(crate) fn be_u16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes(s.try_into().expect("2 bytes")))
}

/// The entry count after a full box's version and flags (`MEDIA_CORRUPT` when absent).
pub(crate) fn full_box_count(body: &[u8], h: &BoxHeader) -> Result<u32, CoreError> {
    be_u32(body, 4).ok_or_else(|| {
        corrupt(
            format!("the {} box is cut short", h.name()),
            h.start,
            &h.name(),
        )
    })
}
