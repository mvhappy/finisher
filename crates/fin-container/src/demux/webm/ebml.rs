//! EBML (RFC 8794) element headers under `fin-container`'s limits: variable-length ids (at most
//! 4 bytes) and sizes (at most 8), a size checked against its parent's end before anything is
//! read, unknown sizes only for `Segment` and `Cluster`, nesting against `max_depth`, elements
//! outside clusters against `max_boxes`, and a body read into memory only within
//! `max_box_bytes`. Header reads are served from a read-ahead window, so a scan over every block
//! does not cost a seek and a read each.

use std::io::{Read, Seek, SeekFrom};

use fin_core::CoreError;

use crate::Limits;
use crate::error::{corrupt, io, limit};

pub(super) const EBML: u32 = 0x1A45_DFA3;
pub(super) const DOC_TYPE: u32 = 0x4282;
pub(super) const DOC_TYPE_READ_VERSION: u32 = 0x4285;
pub(super) const EBML_READ_VERSION: u32 = 0x42F7;
pub(super) const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub(super) const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
pub(super) const SEGMENT: u32 = 0x1853_8067;
pub(super) const SEEK_HEAD: u32 = 0x114D_9B74;
pub(super) const INFO: u32 = 0x1549_A966;
pub(super) const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
pub(super) const TRACKS: u32 = 0x1654_AE6B;
pub(super) const TRACK_ENTRY: u32 = 0xAE;
pub(super) const CLUSTER: u32 = 0x1F43_B675;
pub(super) const CUES: u32 = 0x1C53_BB6B;
pub(super) const CHAPTERS: u32 = 0x1043_A770;
pub(super) const TAGS: u32 = 0x1254_C367;
pub(super) const ATTACHMENTS: u32 = 0x1941_A469;
pub(super) const TIMESTAMP: u32 = 0xE7;
pub(super) const SIMPLE_BLOCK: u32 = 0xA3;
pub(super) const BLOCK_GROUP: u32 = 0xA0;
pub(super) const BLOCK: u32 = 0xA1;
pub(super) const BLOCK_DURATION: u32 = 0x9B;
pub(super) const REFERENCE_BLOCK: u32 = 0xFB;
pub(super) const DISCARD_PADDING: u32 = 0x75A2;

/// The ids that end an unknown-sized `Cluster`: the Segment's other children, another
/// `Segment` and another EBML stream.
pub(super) fn ends_cluster(id: u32) -> bool {
    matches!(
        id,
        CLUSTER | CUES | INFO | TRACKS | SEEK_HEAD | CHAPTERS | TAGS | ATTACHMENTS | SEGMENT | EBML
    )
}

/// A name for error details.
pub(super) fn name(id: u32) -> &'static str {
    match id {
        EBML => "EBML",
        SEGMENT => "Segment",
        SEEK_HEAD => "SeekHead",
        INFO => "Info",
        TRACKS => "Tracks",
        TRACK_ENTRY => "TrackEntry",
        CLUSTER => "Cluster",
        CUES => "Cues",
        SIMPLE_BLOCK => "SimpleBlock",
        BLOCK_GROUP => "BlockGroup",
        BLOCK => "Block",
        TIMESTAMP => "Timestamp",
        0xBB => "CuePoint",
        0xB7 => "CueTrackPositions",
        0x6D80 => "ContentEncodings",
        0x63A2 => "CodecPrivate",
        0xE0 => "Video",
        0xE1 => "Audio",
        0x55B0 => "Colour",
        0x55D0 => "MasteringMetadata",
        0x7670 => "Projection",
        _ => "element",
    }
}

/// An element's place in the file. An unknown size reads as reaching the parent's end until
/// the caller finds where it stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Element {
    pub id: u32,
    pub start: u64,
    pub body: u64,
    pub end: u64,
    pub unknown: bool,
}

impl Element {
    pub fn len(&self) -> u64 {
        self.end - self.body
    }

    pub fn name(&self) -> &'static str {
        name(self.id)
    }
}

const WINDOW: usize = 64 << 10;

pub(super) struct Ebml<'a, R: Read + Seek> {
    r: &'a mut R,
    pub len: u64,
    pub limits: Limits,
    elements: u32,
    window: Vec<u8>,
    window_at: u64,
}

fn cut(at: u64) -> CoreError {
    corrupt("the file ends inside a read", at, "file")
}

impl<'a, R: Read + Seek> Ebml<'a, R> {
    pub fn new(r: &'a mut R, limits: Limits) -> Result<Self, CoreError> {
        let len = r.seek(SeekFrom::End(0)).map_err(|e| io(&e))?;
        Ok(Ebml {
            r,
            len,
            limits,
            elements: 0,
            window: Vec::new(),
            window_at: 0,
        })
    }

    fn raw_read(&mut self, at: u64, buf: &mut [u8]) -> Result<(), CoreError> {
        self.r.seek(SeekFrom::Start(at)).map_err(|e| io(&e))?;
        self.r.read_exact(buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => cut(at),
            _ => io(&e),
        })
    }

    /// Reads exactly `buf.len()` bytes at `at`.
    pub fn read_at(&mut self, at: u64, buf: &mut [u8]) -> Result<(), CoreError> {
        let n = buf.len() as u64;
        if at.checked_add(n).is_none_or(|e| e > self.len) {
            return Err(cut(at));
        }
        if buf.len() > WINDOW / 2 {
            return self.raw_read(at, buf);
        }
        let inside = at >= self.window_at && at + n <= self.window_at + self.window.len() as u64;
        if !inside {
            let take = (self.len - at).min(WINDOW as u64) as usize;
            let mut w = std::mem::take(&mut self.window);
            w.resize(take, 0);
            let read = self.raw_read(at, &mut w);
            self.window = w;
            self.window_at = at;
            if let Err(e) = read {
                self.window.clear();
                return Err(e);
            }
        }
        let from = (at - self.window_at) as usize;
        buf.copy_from_slice(&self.window[from..from + buf.len()]);
        Ok(())
    }

    /// Up to `max` bytes at `at`, fewer when `end` comes first.
    fn peek(&mut self, at: u64, end: u64, max: usize) -> Result<Vec<u8>, CoreError> {
        let n = end.saturating_sub(at).min(max as u64) as usize;
        let mut b = vec![0u8; n];
        self.read_at(at, &mut b)?;
        Ok(b)
    }

    /// The header of the element at `at`, which must end by `parent_end`. `counted` elements
    /// count against `max_boxes` (blocks in clusters do not: they count as samples).
    pub fn header(
        &mut self,
        at: u64,
        parent_end: u64,
        depth: u8,
        counted: bool,
    ) -> Result<Element, CoreError> {
        if counted {
            self.elements += 1;
            if self.elements > self.limits.max_boxes {
                return Err(limit(
                    "maxBoxes",
                    self.elements as u64,
                    self.limits.max_boxes as u64,
                ));
            }
        }
        if depth > self.limits.max_depth {
            return Err(limit(
                "maxDepth",
                depth as u64,
                self.limits.max_depth as u64,
            ));
        }
        let head = self.peek(at, parent_end, 12)?;
        let (id, id_len) = vint(&head, 4)
            .ok_or_else(|| corrupt("an element id is malformed or cut short", at, "element"))?;
        let id = id as u32;
        let (size, size_len) = vint(&head[id_len..], 8)
            .ok_or_else(|| corrupt("an element size is malformed or cut short", at, name(id)))?;
        let body = at + (id_len + size_len) as u64;
        let all_ones = size == (1u64 << (7 * size_len)) - 1;
        if all_ones {
            if !matches!(id, SEGMENT | CLUSTER) {
                return Err(corrupt(
                    format!("the {} element has an unknown size", name(id)),
                    at,
                    name(id),
                ));
            }
            return Ok(Element {
                id,
                start: at,
                body,
                end: parent_end,
                unknown: true,
            });
        }
        let left = parent_end - body;
        if size > left {
            return Err(corrupt(
                format!(
                    "the {} element declares {size} bytes but only {left} are left",
                    name(id)
                ),
                at,
                name(id),
            ));
        }
        Ok(Element {
            id,
            start: at,
            body,
            end: body + size,
            unknown: false,
        })
    }

    /// The children of a known-size master element (each counted). Bytes too short for a
    /// header at the end (a 1-byte `Void` remainder some writers leave) are ignored.
    pub fn children(&mut self, parent: &Element, depth: u8) -> Result<Vec<Element>, CoreError> {
        self.children_of(parent, depth, None)
    }

    /// [`Ebml::children`] not counted against `max_boxes` but at most `max_samples` of them
    /// (`MEDIA_LIMIT maxSamples` past it), so the list stays bounded.
    pub fn children_uncounted(
        &mut self,
        parent: &Element,
        depth: u8,
    ) -> Result<Vec<Element>, CoreError> {
        let cap = self.limits.max_samples as u64;
        self.children_of(parent, depth, Some(cap))
    }

    /// `cap`: `None` counts each child against `max_boxes`; `Some(n)` allows at most `n`.
    fn children_of(
        &mut self,
        parent: &Element,
        depth: u8,
        cap: Option<u64>,
    ) -> Result<Vec<Element>, CoreError> {
        let mut out = Vec::new();
        let mut at = parent.body;
        while parent.end - at >= 2 {
            if let Some(cap) = cap
                && out.len() as u64 >= cap
            {
                return Err(limit("maxSamples", out.len() as u64 + 1, cap));
            }
            let e = self.header(at, parent.end, depth, cap.is_none())?;
            if e.unknown {
                return Err(corrupt(
                    format!("an unknown-sized element inside {}", parent.name()),
                    e.start,
                    e.name(),
                ));
            }
            at = e.end;
            out.push(e);
        }
        Ok(out)
    }

    /// The body of `e`, read whole (`MEDIA_LIMIT` past `max_box_bytes`).
    pub fn body(&mut self, e: &Element) -> Result<Vec<u8>, CoreError> {
        if e.len() > self.limits.max_box_bytes {
            return Err(limit("maxBoxBytes", e.len(), self.limits.max_box_bytes));
        }
        let mut b = vec![0u8; e.len() as usize];
        self.read_at(e.body, &mut b)?;
        Ok(b)
    }

    /// An unsigned integer element (at most 8 bytes; empty is 0).
    pub fn uint(&mut self, e: &Element) -> Result<u64, CoreError> {
        if e.len() > 8 {
            return Err(corrupt(
                format!("the {} integer has {} bytes", e.name(), e.len()),
                e.start,
                e.name(),
            ));
        }
        Ok(self.body(e)?.iter().fold(0u64, |v, &b| (v << 8) | b as u64))
    }

    /// A float element (0, 4 or 8 bytes; empty is 0).
    pub fn float(&mut self, e: &Element) -> Result<f64, CoreError> {
        let b = self.body(e)?;
        match b.len() {
            0 => Ok(0.0),
            4 => Ok(f32::from_be_bytes(b[..4].try_into().expect("4 bytes")) as f64),
            8 => Ok(f64::from_be_bytes(b[..8].try_into().expect("8 bytes"))),
            n => Err(corrupt(
                format!("the {} float has {n} bytes", e.name()),
                e.start,
                e.name(),
            )),
        }
    }

    /// A string element (zero padding after the text is dropped).
    pub fn string(&mut self, e: &Element) -> Result<String, CoreError> {
        let b = self.body(e)?;
        let text = b.split(|&c| c == 0).next().unwrap_or(&[]);
        Ok(String::from_utf8_lossy(text).into_owned())
    }
}

/// A variable-length integer of at most `max` bytes: `(value, length)`, the length marker
/// kept for ids (`max == 4`) and removed for sizes and block track numbers.
pub(super) fn vint(b: &[u8], max: usize) -> Option<(u64, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > max || len > 8 || b.len() < len {
        return None;
    }
    let mut v = if max == 4 {
        first as u64
    } else {
        (first as u64) & (0xFF >> len)
    };
    for &c in &b[1..len] {
        v = (v << 8) | c as u64;
    }
    Some((v, len))
}

/// A signed lacing size difference (EBML lacing): the unsigned value minus half its range.
pub(super) fn svint(b: &[u8]) -> Option<(i64, usize)> {
    let (v, len) = vint(b, 8)?;
    Some((v as i64 - ((1i64 << (7 * len - 1)) - 1), len))
}
