//! The first sample entry of a track's `stsd`: the coded size or audio format, the codec
//! configuration box (kept as bytes for the decoder) and what it says (profile, level, bit
//! depth, chroma), and the colour, aspect and HDR boxes beside it.

use std::io::{Read, Seek};

use fin_core::CoreError;
use fin_media::{Chroma, ChromaSiting, ColourTags, HdrInfo, Size};

use super::colour::{ColrInfo, clli, colr, dovi, mdcv};
use super::reader::{BoxHeader, Reader, be_u16, be_u32};
use crate::CodecConfig;
use crate::error::corrupt;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub fourcc: [u8; 4],
    pub config: CodecConfig,
    /// Video: the coded size.
    pub coded: Option<Size>,
    /// Audio: channels and sample rate.
    pub audio: Option<(u16, u32)>,
    pub profile: Option<u8>,
    pub level: Option<u8>,
    pub bit_depth: u8,
    pub chroma: Chroma,
    pub siting: ChromaSiting,
    pub colr: Option<ColrInfo>,
    /// Colour from the codec configuration (`vpcC`), used when there is no `colr`.
    pub config_colour: Option<ColourTags>,
    pub pasp: Option<(u32, u32)>,
    pub hdr: HdrInfo,
    /// Opus pre-skip from `dOps`.
    pub pre_skip: Option<u16>,
}

/// The bytes of a visual (78) or audio (28, more for QuickTime sound v1/v2) entry header.
fn header_len(body: &[u8], video: bool) -> usize {
    match (video, be_u16(body, 8)) {
        (true, _) => 78,
        (false, Some(1)) => 28 + 16,
        (false, Some(2)) => 28 + 36,
        (false, _) => 28,
    }
}

pub(crate) fn config_info(e: &mut Entry, kind: &[u8; 4], b: &[u8]) {
    match kind {
        b"av1C" if b.len() >= 4 => {
            e.profile = Some(b[1] >> 5);
            e.level = Some(b[1] & 0x1f);
            let (high, twelve, mono, sx, sy) = (
                b[2] & 0x40 != 0,
                b[2] & 0x20 != 0,
                b[2] & 0x10 != 0,
                b[2] & 0x08 != 0,
                b[2] & 0x04 != 0,
            );
            e.bit_depth = if twelve {
                12
            } else if high {
                10
            } else {
                8
            };
            e.chroma = match (mono, sx, sy) {
                (true, _, _) => Chroma::Mono,
                (_, true, true) => Chroma::Yuv420,
                (_, true, false) => Chroma::Yuv422,
                _ => Chroma::Yuv444,
            };
            e.siting = match b[2] & 0x03 {
                1 => ChromaSiting::Left,
                2 => ChromaSiting::TopLeft,
                _ => ChromaSiting::Unknown,
            };
        }
        b"avcC" if b.len() >= 4 => {
            e.profile = Some(b[1]);
            e.level = Some(b[3]);
        }
        b"hvcC" if b.len() >= 18 => {
            e.profile = Some(b[1] & 0x1f);
            e.level = Some(b[12]);
            e.chroma = match b[16] & 0x03 {
                0 => Chroma::Mono,
                2 => Chroma::Yuv422,
                3 => Chroma::Yuv444,
                _ => Chroma::Yuv420,
            };
            e.bit_depth = 8 + (b[17] & 0x07);
        }
        b"vpcC" if b.len() >= 11 => {
            // A full box: version and flags, then profile, level, bit depth and chroma.
            e.profile = Some(b[4]);
            e.level = Some(b[5]);
            e.bit_depth = b[6] >> 4;
            e.chroma = match (b[6] >> 1) & 0x07 {
                0 | 1 => Chroma::Yuv420,
                2 => Chroma::Yuv422,
                _ => Chroma::Yuv444,
            };
            e.config_colour = Some(ColourTags::from_cicp(
                b[7] as u16,
                b[8] as u16,
                b[9] as u16,
                b[6] & 1 != 0,
            ));
        }
        b"dOps" if b.len() >= 4 => {
            e.pre_skip = be_u16(b, 2);
            if let Some((_, rate)) = e.audio {
                e.audio = Some((b[1] as u16, rate));
            }
        }
        _ => {}
    }
}

/// The first entry of `stsd`; `video` says whether the track is a visual track.
pub(crate) fn entry<R: Read + Seek>(
    rd: &mut Reader<R>,
    stsd: &BoxHeader,
    video: bool,
    depth: u8,
) -> Result<Entry, CoreError> {
    if stsd.body_len() < 8 {
        return Err(corrupt("the stsd box is cut short", stsd.start, "stsd"));
    }
    let eh = rd.header(stsd.body + 8, stsd.end, depth + 1)?;
    let body = rd.body(&eh)?;
    let skip = header_len(&body, video);
    if body.len() < skip {
        return Err(corrupt(
            format!("the {} sample entry is cut short", eh.name()),
            eh.start,
            &eh.name(),
        ));
    }
    let mut e = Entry {
        fourcc: eh.kind,
        config: CodecConfig::Other { fourcc: eh.kind },
        coded: None,
        audio: None,
        profile: None,
        level: None,
        bit_depth: 8,
        chroma: Chroma::Yuv420,
        siting: ChromaSiting::Unknown,
        colr: None,
        config_colour: None,
        pasp: None,
        hdr: HdrInfo::default(),
        pre_skip: None,
    };
    if video {
        e.coded = Some(Size::new(
            be_u16(&body, 24).unwrap_or(0) as u32,
            be_u16(&body, 26).unwrap_or(0) as u32,
        ));
    } else {
        let channels = be_u16(&body, 16).unwrap_or(0);
        let rate = match be_u16(&body, 8) {
            // QuickTime sound v2 stores the rate as an f64 at 32; only its integer part matters.
            Some(2) => body.get(32..40).map_or(0, |b| {
                f64::from_be_bytes(b.try_into().expect("8 bytes")) as u32
            }),
            _ => be_u32(&body, 24).unwrap_or(0) >> 16,
        };
        e.audio = Some((channels, rate));
    }
    let mut children = rd.children(eh.body + skip as u64, eh.end, depth + 2)?;
    // QuickTime audio keeps its `esds` inside a `wave` box.
    if let Some(wave) = children.iter().find(|c| &c.kind == b"wave").copied() {
        children.extend(rd.children(wave.body, wave.end, depth + 3)?);
    }
    for c in children {
        let b = rd.body(&c)?;
        match &c.kind {
            b"av1C" => e.config = CodecConfig::Av1 { av1c: b.clone() },
            b"avcC" => e.config = CodecConfig::Avc { avcc: b.clone() },
            b"hvcC" => e.config = CodecConfig::Hevc { hvcc: b.clone() },
            b"vpcC" => e.config = CodecConfig::Vp9 { vpcc: b.clone() },
            b"esds" => e.config = CodecConfig::Aac { esds: b.clone() },
            b"dOps" => e.config = CodecConfig::Opus { dops: b.clone() },
            b"colr" if e.colr.is_none() => e.colr = colr(&b, &c)?,
            b"pasp" => {
                let (h, v) = (be_u32(&b, 0).unwrap_or(0), be_u32(&b, 4).unwrap_or(0));
                e.pasp = (h > 0 && v > 0).then_some((h, v));
            }
            b"mdcv" => e.hdr.mastering = Some(mdcv(&b, &c)?),
            b"clli" => e.hdr.content_light = Some(clli(&b, &c)?),
            b"dvcC" | b"dvvC" | b"dvwC" => e.hdr.dolby_vision = Some(dovi(&b, &c)?),
            _ => {}
        }
        config_info(&mut e, &c.kind, &b);
    }
    Ok(e)
}
