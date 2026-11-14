//! A Matroska track entry as the MP4 demuxer's sample-entry description ([`Entry`]): the
//! `CodecID` as the equivalent ISOBMFF type, `CodecPrivate` as the configuration record (AV1's
//! is `av1C` itself; VP9's feature records become a `vpcC`; `OpusHead` becomes `dOps`; AVC and
//! HEVC carry `avcC`/`hvcC`), and `Colour` as colour tags and HDR metadata.

use fin_core::CoreError;
use fin_media::{Chroma, ChromaSiting, ColourTags, ContentLight, HdrInfo, MasteringDisplay, Size};

use super::segment::{ColourDesc, TrackDesc};
use crate::CodecConfig;
use crate::demux::codec_config::{Entry, config_info};
use crate::demux::colour::ColrInfo;
use crate::error::{corrupt, unsupported_projection};

fn empty(fourcc: [u8; 4]) -> Entry {
    Entry {
        fourcc,
        config: CodecConfig::Other { fourcc },
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
    }
}

fn gcd(a: u128, b: u128) -> u128 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The video track's entry (`UNSUPPORTED_TRANSFORM` for a projection).
pub(super) fn video(t: &TrackDesc) -> Result<Entry, CoreError> {
    let v = t.video.clone().unwrap_or_default();
    if let Some((kind, posed)) = v.projection
        && (kind != 0 || posed)
    {
        return Err(unsupported_projection(kind));
    }
    let private = t.private.as_deref().unwrap_or(&[]);
    let colour = v.colour.clone().unwrap_or_default();
    let mut e = match t.codec_id.as_str() {
        "V_AV1" => record(*b"av01", b"av1C", private, 4, |b| CodecConfig::Av1 {
            av1c: b,
        }),
        "V_MPEG4/ISO/AVC" => record(*b"avc1", b"avcC", private, 4, |b| CodecConfig::Avc {
            avcc: b,
        }),
        "V_MPEGH/ISO/HEVC" => record(*b"hvc1", b"hvcC", private, 18, |b| CodecConfig::Hevc {
            hvcc: b,
        }),
        "V_VP9" => {
            let vpcc = vpcc(private, &colour);
            let mut e = record(*b"vp09", b"vpcC", &vpcc, 11, |b| CodecConfig::Vp9 {
                vpcc: b,
            });
            e.level = e.level.filter(|&l| l > 0);
            e
        }
        "V_VP8" => empty(*b"vp08"),
        "V_PRORES" => {
            let fourcc: [u8; 4] = private.try_into().unwrap_or(*b"apcn");
            match fin_media::VideoCodec::from_fourcc(&fourcc) {
                fin_media::VideoCodec::ProRes => empty(fourcc),
                _ => empty(*b"apcn"),
            }
        }
        _ => empty(*b"mkvv"),
    };
    let size =
        |n: u64| u32::try_from(n).map_err(|_| corrupt("the video size is over 2³²", t.at, "Video"));
    e.coded = Some(Size::new(size(v.width)?, size(v.height)?));
    if let Some((dw, dh)) = v.display
        && dw > 0
        && dh > 0
        && v.width > 0
        && v.height > 0
        && (dw, dh) != (v.width, v.height)
    {
        let (h, w) = (dw as u128 * v.height as u128, dh as u128 * v.width as u128);
        let g = gcd(h, w);
        if let (Ok(h), Ok(w)) = (u32::try_from(h / g), u32::try_from(w / g)) {
            e.pasp = Some((h, w));
        }
    }
    apply_colour(&mut e, &colour, v.colour.is_some());
    Ok(e)
}

/// An entry whose `CodecPrivate` is the configuration record `kind` (when long enough).
fn record(
    fourcc: [u8; 4],
    kind: &[u8; 4],
    private: &[u8],
    min: usize,
    config: impl Fn(Vec<u8>) -> CodecConfig,
) -> Entry {
    let mut e = empty(fourcc);
    if private.len() >= min {
        e.config = config(private.to_vec());
        config_info(&mut e, kind, private);
    }
    e
}

/// VP9's `CodecPrivate` feature records (id, length, value: 1 profile, 2 level, 3 bit depth,
/// 4 chroma subsampling) and `Colour`, as a `vpcC` body (version 1).
fn vpcc(private: &[u8], c: &ColourDesc) -> Vec<u8> {
    let mut f = [None::<u8>; 5];
    let mut at = 0;
    while let (Some(&id), Some(&len)) = (private.get(at), private.get(at + 1)) {
        let Some(value) = private.get(at + 2..at + 2 + len as usize) else {
            break;
        };
        if (1..=4).contains(&id) && len == 1 {
            f[id as usize] = Some(value[0]);
        }
        at += 2 + len as usize;
    }
    let bits = f[3]
        .or(c.bits.and_then(|b| u8::try_from(b).ok()).filter(|&b| b > 0))
        .unwrap_or(8);
    let chroma = f[4].unwrap_or(match c.subsampling {
        (Some(1), Some(0)) => 2,
        (Some(0), Some(0)) => 3,
        _ => 1,
    });
    let profile = f[1].unwrap_or(match (bits > 8, chroma >= 2) {
        (false, false) => 0,
        (false, true) => 1,
        (true, false) => 2,
        (true, true) => 3,
    });
    let code = |v: Option<u64>| v.and_then(|v| u8::try_from(v).ok()).unwrap_or(2);
    let full = u8::from(c.range == Some(2));
    vec![
        1,
        0,
        0,
        0,
        profile,
        f[2].unwrap_or(0),
        (bits << 4) | ((chroma & 7) << 1) | full,
        code(c.primaries),
        code(c.transfer),
        code(c.matrix),
        0,
        0,
    ]
}

/// `Colour` over what the codec record said: code points (as a `colr` would be), siting, bit
/// depth when there is no record, content light and mastering display.
fn apply_colour(e: &mut Entry, c: &ColourDesc, present: bool) {
    if !present {
        return;
    }
    if c.primaries.is_some() || c.transfer.is_some() || c.matrix.is_some() || c.range.is_some() {
        let code = |v: Option<u64>| v.and_then(|v| u16::try_from(v).ok()).unwrap_or(2);
        e.colr = Some(ColrInfo::Tags(ColourTags::from_cicp(
            code(c.primaries),
            code(c.transfer),
            code(c.matrix),
            c.range == Some(2),
        )));
    }
    e.siting = match c.siting {
        (Some(1), Some(1)) => ChromaSiting::TopLeft,
        (Some(1), Some(2)) => ChromaSiting::Left,
        (Some(2), Some(2)) => ChromaSiting::Center,
        _ => e.siting,
    };
    if matches!(e.config, CodecConfig::Other { .. })
        && let Some(b) = c.bits.and_then(|b| u8::try_from(b).ok()).filter(|&b| b > 0)
    {
        e.bit_depth = b;
    }
    if c.max_cll.is_some() || c.max_fall.is_some() {
        let n = |v: Option<u64>| v.unwrap_or(0).min(u16::MAX as u64) as u16;
        e.hdr.content_light = Some(ContentLight {
            max_cll: n(c.max_cll),
            max_fall: n(c.max_fall),
        });
    }
    if let Some(m) = c.mastering
        && m.iter().all(|v| v.is_finite() && *v >= 0.0)
    {
        let xy = |v: f64| (v * 50_000.0).round().min(u16::MAX as f64) as u16;
        let lum = |v: f64| (v * 10_000.0).round().min(u32::MAX as f64) as u32;
        e.hdr.mastering = Some(MasteringDisplay {
            // mdcv's order: green, blue, red.
            primaries: [
                [xy(m[2]), xy(m[3])],
                [xy(m[4]), xy(m[5])],
                [xy(m[0]), xy(m[1])],
            ],
            white_point: [xy(m[6]), xy(m[7])],
            max_luminance: lum(m[8]),
            min_luminance: lum(m[9]),
        });
    }
}

/// The audio track's entry.
pub(super) fn audio(t: &TrackDesc) -> Entry {
    let a = t.audio.clone();
    let id = t.codec_id.as_str();
    let fourcc = match id {
        "A_OPUS" => *b"Opus",
        "A_FLAC" => *b"fLaC",
        "A_MPEG/L3" => *b".mp3",
        "A_AC3" => *b"ac-3",
        "A_EAC3" => *b"ec-3",
        "A_PCM/INT/LIT" => *b"sowt",
        "A_PCM/INT/BIG" => *b"twos",
        "A_PCM/FLOAT/IEEE" => *b"fl32",
        "A_VORBIS" => *b"vorb",
        _ if id == "A_AAC" || id.starts_with("A_AAC/") => *b"mp4a",
        _ => *b"mkva",
    };
    let mut e = empty(fourcc);
    let (rate, channels) = a.map_or((0.0, 0), |a| (a.rate, a.channels));
    let rate = if rate.is_finite() && rate > 0.0 {
        rate.round().min(u32::MAX as f64) as u32
    } else {
        0
    };
    e.audio = Some((channels.min(u16::MAX as u64) as u16, rate));
    if fourcc == *b"Opus"
        && let Some(dops) = t.private.as_deref().and_then(dops)
    {
        config_info(&mut e, b"dOps", &dops);
        e.config = CodecConfig::Opus { dops };
    }
    e
}

/// `OpusHead` (RFC 7845 §5.1, little-endian) as a `dOps` body (big-endian, no magic).
fn dops(head: &[u8]) -> Option<Vec<u8>> {
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return None;
    }
    let mut d = vec![0, head[9]];
    d.extend(u16::from_le_bytes([head[10], head[11]]).to_be_bytes());
    d.extend(u32::from_le_bytes(head[12..16].try_into().expect("4 bytes")).to_be_bytes());
    d.extend(i16::from_le_bytes([head[16], head[17]]).to_be_bytes());
    d.push(head[18]);
    if head[18] != 0 {
        // Stream count, coupled count, then one mapping byte a channel.
        let table = head.get(19..21 + head[9] as usize)?;
        d.extend(table);
    }
    Some(d)
}
