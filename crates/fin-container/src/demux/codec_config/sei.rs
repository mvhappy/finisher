//! In-band HDR metadata in H.264 and HEVC SEI (plan 6B-1 Task 11; sub-project 6 spec §6.3,
//! §10.1): the mastering display colour volume (payload type 137, the fields of SMPTE ST 2086
//! in `mdcv`'s order and units) and the content light level (144, `clli`'s). The deep probe
//! reads them from a track's first sample when the sample entry has no `mdcv`/`clli` box.
//!
//! This is a parser of SEI framing only, not of the codec: it walks the length-prefixed NAL
//! units of one sample (the length size from `avcC`/`hvcC`), keeps the SEI ones (H.264 type 6,
//! HEVC prefix 39 and suffix 40), removes emulation prevention bytes and reads the message
//! headers. Anything malformed (a length past the sample, a payload past its NAL, a short
//! payload) ends the walk of that unit and is otherwise ignored: in-band metadata is a hint,
//! and a damaged one is no reason to fail a probe. Never a panic; at most one NAL unit's bytes
//! are copied at a time.

use std::io::{Read, Seek};

use fin_core::CoreError;
use fin_media::{ContentLight, HdrInfo, MasteringDisplay};

use crate::CodecConfig;
use crate::demux::Demuxer;

const MASTERING_DISPLAY: u32 = 137;
const CONTENT_LIGHT: u32 = 144;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Avc,
    Hevc,
}

/// The codec and its NAL length size (1, 2 or 4 bytes; 3 is reserved and refused).
fn syntax(config: &CodecConfig) -> Option<(Syntax, usize)> {
    let (syntax, byte) = match config {
        CodecConfig::Avc { avcc } => (Syntax::Avc, *avcc.get(4)?),
        CodecConfig::Hevc { hvcc } => (Syntax::Hevc, *hvcc.get(21)?),
        _ => return None,
    };
    match (byte & 0x03) + 1 {
        3 => None,
        n => Some((syntax, n as usize)),
    }
}

/// The HDR metadata the SEI messages of `sample` (one access unit of a track whose record is
/// `config`) carry, or `None` when the codec has no SEI (AV1, VP9, ProRes) or the sample holds
/// neither message. A message seen twice keeps its first value.
pub fn sei_hdr(config: &CodecConfig, sample: &[u8]) -> Option<HdrInfo> {
    let (syntax, len_size) = syntax(config)?;
    let mut hdr = HdrInfo::default();
    let mut at = 0usize;
    while at + len_size <= sample.len() {
        let len = sample[at..at + len_size]
            .iter()
            .fold(0usize, |n, &b| (n << 8) | b as usize);
        at += len_size;
        let Some(nal) = at.checked_add(len).and_then(|end| sample.get(at..end)) else {
            break;
        };
        at += len;
        let (kind, header) = match syntax {
            Syntax::Avc => (nal.first().map(|b| b & 0x1F), 1),
            Syntax::Hevc => (nal.first().map(|b| (b >> 1) & 0x3F), 2),
        };
        let is_sei = match syntax {
            Syntax::Avc => kind == Some(6),
            Syntax::Hevc => matches!(kind, Some(39 | 40)),
        };
        if is_sei && nal.len() > header {
            messages(&unescape(&nal[header..]), &mut hdr);
        }
    }
    (!hdr.is_empty()).then_some(hdr)
}

/// The RBSP of a NAL unit's payload: every `00 00 03` loses its `03`.
fn unescape(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut zeros = 0;
    for &b in ebsp {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// One `ff`-extended SEI header value (type or size) at `*at`.
fn value(rbsp: &[u8], at: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    loop {
        let b = *rbsp.get(*at)?;
        *at += 1;
        v = v.checked_add(b as u32)?;
        if b != 0xFF {
            return Some(v);
        }
    }
}

/// The SEI messages of one RBSP, until its trailing bits or the first malformed header.
fn messages(rbsp: &[u8], hdr: &mut HdrInfo) {
    let mut at = 0usize;
    // `more_rbsp_data`: stop at the `rbsp_trailing_bits` byte (`0x80`) that ends the unit.
    while at < rbsp.len() && !(at + 1 == rbsp.len() && rbsp[at] == 0x80) {
        let (Some(kind), Some(size)) = (value(rbsp, &mut at), value(rbsp, &mut at)) else {
            return;
        };
        let Some(payload) = at
            .checked_add(size as usize)
            .and_then(|end| rbsp.get(at..end))
        else {
            return;
        };
        at += size as usize;
        match kind {
            MASTERING_DISPLAY if hdr.mastering.is_none() => hdr.mastering = mastering(payload),
            CONTENT_LIGHT if hdr.content_light.is_none() => hdr.content_light = light(payload),
            _ => {}
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `mastering_display_colour_volume`: three primaries (G, B, R as `mdcv`), the white point,
/// the maximum and minimum luminance (0.0001 cd/m²). A payload under 24 bytes is ignored.
fn mastering(p: &[u8]) -> Option<MasteringDisplay> {
    let u = |at| u16_at(p, at);
    Some(MasteringDisplay {
        primaries: [[u(0)?, u(2)?], [u(4)?, u(6)?], [u(8)?, u(10)?]],
        white_point: [u(12)?, u(14)?],
        max_luminance: u32_at(p, 16)?,
        min_luminance: u32_at(p, 20)?,
    })
}

/// `content_light_level_info`: MaxCLL and MaxFALL in cd/m².
fn light(p: &[u8]) -> Option<ContentLight> {
    Some(ContentLight {
        max_cll: u16_at(p, 0)?,
        max_fall: u16_at(p, 2)?,
    })
}

/// [`sei_hdr`] of the video track's first sample in decode order (where encoders put these
/// messages: with the first IDR/IRAP picture), or `None` without a video track, a sample or
/// a message. The sample is read under the demuxer's limits (`MEDIA_LIMIT`, `MEDIA_CORRUPT`).
pub fn in_band_hdr<R: Read + Seek>(d: &mut Demuxer<R>) -> Result<Option<HdrInfo>, CoreError> {
    let Some(v) = d.video() else {
        return Ok(None);
    };
    if syntax(&v.config).is_none() || v.sample_count() == 0 {
        return Ok(None);
    }
    let (track, config) = (v.track, v.config.clone());
    let mut buf = Vec::new();
    d.read_sample(track, 0, &mut buf)?;
    Ok(sei_hdr(&config, &buf))
}
