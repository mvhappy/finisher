//! The ring file layout (spec §3.4). All integers are little-endian.
//!
//! ```text
//! 0   magic      "FINSHM01"
//! 8   version    u32 (1)
//! 12  reserved   u32 (0)
//! 16  slot_bytes u64 (≥ 1)
//! 24  slots      u32 (1..=MAX_SLOTS)
//! 28  reserved   u32 (0)
//! 32  slot table: per slot { seq u64, len u64 }
//! data_offset (the table's end rounded up to DATA_ALIGN): slot i at data_offset + i × slot_bytes
//! ```
//!
//! The file is exactly [`RingLayout::file_bytes`] long. `seq == 0` is a slot never written; an
//! odd `seq` a slot being written (or abandoned mid-write); an even one a finished slot whose
//! payload is its first `len` bytes.

use crate::checks::{Fail, Reason};

pub const MAGIC: [u8; 8] = *b"FINSHM01";
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: u64 = 32;
/// One slot table entry: `seq` then `len`.
pub const SLOT_ENTRY_BYTES: u64 = 16;
/// Slots start on a 64-byte boundary.
pub const DATA_ALIGN: u64 = 64;
/// At most 65,536 slots (a 1 MiB table).
pub const MAX_SLOTS: u32 = 1 << 16;

/// A ring's shape: `slots` slots of `slot_bytes` each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingSpec {
    pub slot_bytes: u64,
    pub slots: u32,
}

/// Where everything is in a ring file of a given [`RingSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingLayout {
    pub spec: RingSpec,
    pub data_offset: u64,
    pub file_bytes: u64,
}

impl RingSpec {
    /// The layout, or `Header` when the spec is out of range or its size overflows `u64`.
    pub fn layout(&self) -> Result<RingLayout, Reason> {
        self.checked_layout().map_err(|f| f.reason)
    }

    /// The file size a ring of this spec has (what a kind declares as its `MappedFile.bytes`).
    pub fn file_bytes(&self) -> Option<u64> {
        self.checked_layout().ok().map(|l| l.file_bytes)
    }

    pub(crate) fn checked_layout(&self) -> Result<RingLayout, Fail> {
        if self.slots == 0 || self.slots > MAX_SLOTS {
            return Err(Fail::new(
                Reason::Header,
                format!("a ring has 1 to {MAX_SLOTS} slots, not {}", self.slots),
            ));
        }
        if self.slot_bytes == 0 {
            return Err(Fail::new(
                Reason::Header,
                "a ring's slots are at least one byte",
            ));
        }
        // The table is at most 1 MiB (MAX_SLOTS × 16), so only the data part can overflow.
        let table_end = HEADER_BYTES + u64::from(self.slots) * SLOT_ENTRY_BYTES;
        let data_offset = table_end.div_ceil(DATA_ALIGN) * DATA_ALIGN;
        let file_bytes = u64::from(self.slots)
            .checked_mul(self.slot_bytes)
            .and_then(|d| d.checked_add(data_offset))
            .ok_or_else(|| Fail::new(Reason::Header, "slots × slot_bytes overflows"))?;
        Ok(RingLayout {
            spec: *self,
            data_offset,
            file_bytes,
        })
    }
}

impl RingLayout {
    /// The offset of slot `i`'s `seq` (its `len` follows at `+ 8`).
    pub fn seq_offset(&self, i: u32) -> u64 {
        HEADER_BYTES + u64::from(i) * SLOT_ENTRY_BYTES
    }

    /// The offset of slot `i`'s payload.
    pub fn slot_offset(&self, i: u32) -> u64 {
        // Cannot overflow: `checked_layout` proved `data_offset + slots × slot_bytes` fits.
        self.data_offset + u64::from(i) * self.spec.slot_bytes
    }

    /// `i` is a slot of this ring, else a `Protocol` failure.
    pub(crate) fn check_index(&self, i: u32) -> Result<(), Fail> {
        if i < self.spec.slots {
            Ok(())
        } else {
            Err(Fail::new(
                Reason::Protocol,
                format!(
                    "slot {i} is out of range (the ring has {})",
                    self.spec.slots
                ),
            ))
        }
    }
}

/// The 32-byte header of a ring of `spec`.
pub fn encode_header(spec: &RingSpec) -> [u8; HEADER_BYTES as usize] {
    let mut h = [0u8; HEADER_BYTES as usize];
    h[0..8].copy_from_slice(&MAGIC);
    h[8..12].copy_from_slice(&VERSION.to_le_bytes());
    h[16..24].copy_from_slice(&spec.slot_bytes.to_le_bytes());
    h[24..28].copy_from_slice(&spec.slots.to_le_bytes());
    h
}

/// Decodes a ring header from the first [`HEADER_BYTES`] of `bytes` (more are ignored), and
/// checks that its layout fits in a `u64`. Never panics (fuzzed: `fuzz/`).
pub fn decode_header(bytes: &[u8]) -> Result<RingLayout, Reason> {
    decode(bytes).map_err(|f| f.reason)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<RingLayout, Fail> {
    let Some(h) = bytes.get(..HEADER_BYTES as usize) else {
        return Err(Fail::new(
            Reason::Header,
            format!(
                "a ring header has {HEADER_BYTES} bytes, the file has {}",
                bytes.len()
            ),
        ));
    };
    if h[0..8] != MAGIC {
        return Err(Fail::new(
            Reason::Header,
            "the file is not a fin-shm ring (bad magic)",
        ));
    }
    let u32_at = |o: usize| u32::from_le_bytes([h[o], h[o + 1], h[o + 2], h[o + 3]]);
    let version = u32_at(8);
    if version != VERSION {
        return Err(Fail::new(
            Reason::Header,
            format!("ring version {version}; this build reads version {VERSION}"),
        ));
    }
    if u32_at(12) != 0 || u32_at(28) != 0 {
        return Err(Fail::new(
            Reason::Header,
            "the ring header's reserved fields are not zero",
        ));
    }
    let mut sb = [0u8; 8];
    sb.copy_from_slice(&h[16..24]);
    let spec = RingSpec {
        slot_bytes: u64::from_le_bytes(sb),
        slots: u32_at(24),
    };
    spec.checked_layout()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips() {
        let spec = RingSpec {
            slot_bytes: 3_110_400,
            slots: 4,
        };
        let l = decode_header(&encode_header(&spec)).unwrap();
        assert_eq!(l.spec, spec);
        assert_eq!(l.data_offset, 128);
        assert_eq!(l.file_bytes, 128 + 4 * 3_110_400);
        assert_eq!(spec.file_bytes(), Some(l.file_bytes));
    }

    #[test]
    fn the_data_starts_aligned_after_the_table() {
        for slots in [1, 2, 3, 4, 5, 100, MAX_SLOTS] {
            let l = RingSpec {
                slot_bytes: 1,
                slots,
            }
            .layout()
            .unwrap();
            assert_eq!(l.data_offset % DATA_ALIGN, 0);
            assert!(l.data_offset >= HEADER_BYTES + u64::from(slots) * SLOT_ENTRY_BYTES);
            assert!(
                l.data_offset < HEADER_BYTES + u64::from(slots) * SLOT_ENTRY_BYTES + DATA_ALIGN
            );
        }
    }

    #[test]
    fn an_overflowing_size_is_refused() {
        let spec = RingSpec {
            slot_bytes: u64::MAX / 2,
            slots: 3,
        };
        assert_eq!(decode_header(&encode_header(&spec)), Err(Reason::Header));
        assert_eq!(spec.file_bytes(), None);
        // The largest data part that still fits, and one byte more.
        let fits = RingSpec {
            slot_bytes: u64::MAX - 64,
            slots: 1,
        };
        assert_eq!(fits.file_bytes(), Some(u64::MAX));
        let over = RingSpec {
            slot_bytes: u64::MAX - 63,
            slots: 1,
        };
        assert_eq!(over.file_bytes(), None);
    }

    #[test]
    fn malformed_headers_are_refused() {
        let good = encode_header(&RingSpec {
            slot_bytes: 8,
            slots: 2,
        });
        let bad = |f: &dyn Fn(&mut [u8; 32])| {
            let mut h = good;
            f(&mut h);
            decode_header(&h)
        };
        assert_eq!(decode_header(&good[..31]), Err(Reason::Header));
        assert_eq!(bad(&|h| h[0] = b'X'), Err(Reason::Header));
        assert_eq!(bad(&|h| h[8] = 2), Err(Reason::Header));
        assert_eq!(bad(&|h| h[12] = 1), Err(Reason::Header));
        assert_eq!(bad(&|h| h[31] = 1), Err(Reason::Header));
        assert_eq!(bad(&|h| h[16..24].fill(0)), Err(Reason::Header));
        assert_eq!(bad(&|h| h[24..28].fill(0)), Err(Reason::Header));
        assert_eq!(
            bad(&|h| h[24..28].copy_from_slice(&(MAX_SLOTS + 1).to_le_bytes())),
            Err(Reason::Header)
        );
    }

    /// The fuzz target's properties (`fuzz/fuzz_targets/ring_header.rs`) over mutated valid
    /// headers: a decoded header re-encodes to its bytes and its layout is consistent.
    #[test]
    fn mutated_headers_decode_consistently_or_are_refused() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let (mut ok, mut refused) = (0, 0);
        for _ in 0..200_000 {
            let slots = (next() % u64::from(MAX_SLOTS + 2)) as u32;
            let slot_bytes = match next() % 3 {
                0 => next(),
                1 => next() % 4096,
                _ => u64::MAX - next() % 1024,
            };
            let mut h = encode_header(&RingSpec { slot_bytes, slots });
            if next() % 4 == 0 {
                let at = (next() % 32) as usize;
                h[at] ^= 1 << (next() % 8);
            }
            match decode_header(&h) {
                Err(r) => {
                    assert_eq!(r, Reason::Header);
                    refused += 1;
                }
                Ok(l) => {
                    assert_eq!(encode_header(&l.spec), h);
                    let last = l.spec.slots - 1;
                    assert_eq!(l.data_offset % DATA_ALIGN, 0);
                    assert!(l.seq_offset(last) + SLOT_ENTRY_BYTES <= l.data_offset);
                    assert_eq!(l.slot_offset(last) + l.spec.slot_bytes, l.file_bytes);
                    ok += 1;
                }
            }
        }
        assert!(ok > 1000 && refused > 1000, "ok {ok}, refused {refused}");
    }
}
