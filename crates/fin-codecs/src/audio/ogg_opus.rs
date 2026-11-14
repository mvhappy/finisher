//! Ogg Opus timing (RFC 7845 §4) that symphonia's Ogg reader gets wrong for us: packet
//! durations from the TOC (RFC 6716 §3.1, a code 3 packet's six-bit frame count) and the
//! stream's start offset, the first audio page's granule position minus the samples of the
//! packets that end on it. The reader trims from these (`start + pre-skip` to the last granule)
//! instead of symphonia's per-packet trims.

use std::io::{Read, Seek, SeekFrom};

use fin_core::CoreError;
use serde_json::json;

/// The longest Opus packet: 120 ms at 48 kHz (RFC 6716 §3.2.5).
const MAX_PACKET: u64 = 5760;

/// The samples at 48 kHz of a packet whose first bytes are `toc` and `count` (the frame count
/// byte of a code 3 packet); `None` for a packet RFC 6716 forbids.
pub(crate) fn packet_samples(toc: u8, count: Option<u8>) -> Option<u64> {
    let config = toc >> 3;
    let frame: u64 = match config {
        // SILK: 10, 20, 40, 60 ms.
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        // Hybrid: 10, 20 ms.
        12..=15 => [480, 960][(config % 2) as usize],
        // CELT: 2.5, 5, 10, 20 ms.
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (count? & 0x3F) as u64,
    };
    let n = frame * frames;
    (frames > 0 && n <= MAX_PACKET).then_some(n)
}

/// Where an Ogg Opus stream starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpusStart {
    /// The logical stream's serial number (symphonia's track id).
    pub(crate) serial: u32,
    /// The granule position of the first sample decoded (pre-skip included).
    pub(crate) offset: u64,
}

fn corrupt(what: &str, at: u64) -> CoreError {
    CoreError::new("MEDIA_CORRUPT", format!("the Ogg Opus stream {what}"))
        .with_details(json!({ "at": at, "format": "ogg" }))
}

fn read_exact_at<R: Read + Seek>(r: &mut R, at: u64, buf: &mut [u8]) -> Result<bool, CoreError> {
    let io = |e: std::io::Error| fin_container::error::io(&e);
    r.seek(SeekFrom::Start(at)).map_err(io)?;
    match r.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(io(e)),
    }
}

/// The start of the first Opus stream of an Ogg file (`None` when none of its BOS pages is
/// Opus's): its pages are walked by their headers from the file's start until an audio packet
/// ends, reading only each audio packet's first two bytes. The first audio page must hold no
/// more samples than its granule position, unless it is also the last page (RFC 7845 §4.5).
pub(crate) fn start<R: Read + Seek>(r: &mut R, len: u64) -> Result<Option<OpusStart>, CoreError> {
    let mut serial = None;
    // The current packet's index in the stream (0 is `OpusHead`, 1 `OpusTags`), its first
    // bytes and how many of them were read; whether a packet is open across pages (its start
    // was seen, its end not yet), and the last page's sequence number.
    let (mut index, mut head, mut have) = (0u64, [0u8; 2], 0usize);
    let (mut open, mut last_seq) = (false, None::<u32>);
    let mut at = 0u64;
    while at < len {
        let mut h = [0u8; 27];
        if !read_exact_at(r, at, &mut h)? || &h[..4] != b"OggS" {
            return Err(corrupt("has a page that does not start with `OggS`", at));
        }
        let flags = h[5];
        let granule = i64::from_le_bytes(h[6..14].try_into().expect("8 bytes"));
        let page_serial = u32::from_le_bytes(h[14..18].try_into().expect("4 bytes"));
        let seq = u32::from_le_bytes(h[18..22].try_into().expect("4 bytes"));
        let mut lacing = vec![0u8; h[26] as usize];
        if !read_exact_at(r, at + 27, &mut lacing)? {
            return Err(corrupt("is cut inside a page header", at));
        }
        let body = at + 27 + lacing.len() as u64;
        let next = body + lacing.iter().map(|&l| l as u64).sum::<u64>();
        if serial.is_none() {
            if flags & 2 == 0 {
                // Every BOS page comes first (RFC 3533 §4): no Opus stream.
                return Ok(None);
            }
            let mut magic = [0u8; 8];
            if read_exact_at(r, body, &mut magic)? && &magic == b"OpusHead" {
                serial = Some(page_serial);
            }
        }
        if serial != Some(page_serial) {
            at = next;
            continue;
        }
        // As symphonia's reader does (RFC 7845 §3): a gap in the page sequence or a page that
        // does not continue drops the open packet; a continuation with no open packet starts
        // with an orphaned fragment, skipped up to its end and never counted.
        if last_seq.is_some_and(|s| seq != s.wrapping_add(1)) || flags & 1 == 0 {
            open = false;
            have = 0;
        }
        last_seq = Some(seq);
        let mut orphan = flags & 1 != 0 && !open;
        let (mut off, mut samples, mut ended) = (body, 0u64, false);
        for &l in &lacing {
            let l = l as u64;
            if orphan {
                off += l;
                orphan = l == 255;
                continue;
            }
            if index >= 2 && have < 2 {
                let n = (2 - have).min(l as usize);
                if !read_exact_at(r, off, &mut head[have..have + n])? {
                    return Err(corrupt("is cut inside a page", off));
                }
                have += n;
            }
            off += l;
            if l < 255 {
                if index >= 2 {
                    if have == 0 {
                        return Err(corrupt("has an empty audio packet", at));
                    }
                    let count = (have == 2).then_some(head[1]);
                    samples += packet_samples(head[0], count)
                        .ok_or_else(|| corrupt("has an audio packet RFC 6716 forbids", at))?;
                    ended = true;
                }
                index += 1;
                have = 0;
                open = false;
            } else {
                open = true;
            }
        }
        if orphan {
            // The fragment runs past this page: symphonia ignores the page, and so does the
            // next one's continuation (no packet is open).
            open = false;
        }
        if ended {
            let Ok(granule) = u64::try_from(granule) else {
                return Err(corrupt(
                    "has no granule position on its first audio page",
                    at,
                ));
            };
            return match granule.checked_sub(samples) {
                Some(offset) => Ok(Some(OpusStart {
                    serial: page_serial,
                    offset,
                })),
                // A one-page stream's granule is its length: the rest is end trim.
                None if flags & 4 != 0 => Ok(Some(OpusStart {
                    serial: page_serial,
                    offset: 0,
                })),
                None => Err(corrupt(
                    "states fewer samples on its first audio page than the page holds",
                    at,
                )
                .with_details(json!({
                    "at": at, "format": "ogg", "granule": granule, "samples": samples,
                }))),
            };
        }
        at = next;
    }
    Ok(serial.map(|serial| OpusStart { serial, offset: 0 }))
}

#[cfg(test)]
mod tests {
    use super::packet_samples;

    #[test]
    fn toc_durations() {
        // SILK 10 ms code 0, Hybrid 20 ms code 1, CELT 20 ms code 2.
        assert_eq!(packet_samples(0x00, None), Some(480));
        assert_eq!(packet_samples(0x69, None), Some(1920));
        assert_eq!(packet_samples(0xFA, None), Some(1920));
        // Code 3: the six-bit count, at most 120 ms, never zero frames.
        assert_eq!(packet_samples(0xE3, Some(48)), Some(5760));
        assert_eq!(packet_samples(0xE3, Some(0xC0 | 48)), Some(5760));
        assert_eq!(packet_samples(0xE3, Some(49)), None);
        assert_eq!(packet_samples(0x1B, Some(3)), None);
        assert_eq!(packet_samples(0x1B, Some(2)), Some(5760));
        assert_eq!(packet_samples(0xE3, Some(0)), None);
        assert_eq!(packet_samples(0xE3, None), None);
    }
}
