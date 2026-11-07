//! Sample entries: the visual and audio headers and configuration boxes through `mp4-atom`, and
//! the HDR boxes it has no types for (`mdcv`, `clli`, `dvcC`/`dvvC`), nested here.

use fin_core::{CoreError, Rational};
use fin_media::{HdrInfo, display_size};
use mp4_atom::{
    Atom, Audio, Av1c, Avcc, Colr, Compressor, Dops, Encode, Esds, FixedPoint, Hvcc, Pasp, Visual,
    VpcC,
};
use serde_json::json;

use super::{AudioTrackConfig, CodecConfig, VideoTrackConfig};
use crate::error::{atom, invalid};

/// A box: 32-bit size (64-bit when it does not fit), type, body.
pub(crate) fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let len = body.len() as u64 + 8;
    let mut out = Vec::with_capacity(body.len() + 16);
    if len <= u32::MAX as u64 {
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(kind);
    } else {
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(&(len + 8).to_be_bytes());
    }
    out.extend_from_slice(body);
    out
}

/// `t` serialised by `mp4-atom`.
pub(crate) fn enc<T: Encode>(t: &T, kind: &str) -> Result<Vec<u8>, CoreError> {
    let mut out = Vec::new();
    t.encode(&mut out).map_err(|e| atom(e, 0, kind))?;
    Ok(out)
}

/// A configuration record decoded (validated) and re-encoded by `mp4-atom`.
fn config<T: Atom + Encode>(body: &[u8], kind: &str) -> Result<Vec<u8>, CoreError> {
    let mut buf = body;
    let parsed = T::decode_body(&mut buf).map_err(|e| {
        invalid(
            format!("the {kind} configuration does not parse: {e}"),
            json!({ "box": kind }),
        )
    })?;
    enc(&parsed, kind)
}

fn mdcv(m: &fin_media::MasteringDisplay) -> Vec<u8> {
    let mut b = Vec::with_capacity(24);
    for [x, y] in m.primaries {
        b.extend_from_slice(&x.to_be_bytes());
        b.extend_from_slice(&y.to_be_bytes());
    }
    b.extend_from_slice(&m.white_point[0].to_be_bytes());
    b.extend_from_slice(&m.white_point[1].to_be_bytes());
    b.extend_from_slice(&m.max_luminance.to_be_bytes());
    b.extend_from_slice(&m.min_luminance.to_be_bytes());
    boxed(b"mdcv", &b)
}

fn clli(c: &fin_media::ContentLight) -> Vec<u8> {
    let mut b = c.max_cll.to_be_bytes().to_vec();
    b.extend_from_slice(&c.max_fall.to_be_bytes());
    boxed(b"clli", &b)
}

/// A Dolby Vision configuration record (`dvcC` for profiles up to 7, `dvvC` from 8).
fn dovi(d: &fin_media::DolbyVision) -> Vec<u8> {
    let mut b = vec![0u8; 24];
    b[0] = 1;
    b[2] = (d.profile << 1) | (d.level >> 5);
    b[3] = ((d.level & 0x1f) << 3)
        | ((d.rpu as u8) << 2)
        | ((d.enhancement_layer as u8) << 1)
        | d.base_layer as u8;
    b[4] = d.compatibility << 4;
    boxed(if d.profile <= 7 { b"dvcC" } else { b"dvvC" }, &b)
}

fn hdr_boxes(hdr: &HdrInfo, out: &mut Vec<u8>) {
    if let Some(m) = &hdr.mastering {
        out.extend(mdcv(m));
    }
    if let Some(c) = &hdr.content_light {
        out.extend(clli(c));
    }
    if let Some(d) = &hdr.dolby_vision {
        out.extend(dovi(d));
    }
}

/// The visual sample entry of `t`.
pub(crate) fn video_entry(
    t: &VideoTrackConfig,
    #[cfg(feature = "testing")] knobs: &super::knobs::TrackKnobs,
) -> Result<Vec<u8>, CoreError> {
    let visual = Visual {
        data_reference_index: 1,
        width: t.coded.width as u16,
        height: t.coded.height as u16,
        horizresolution: FixedPoint::new(72, 0),
        vertresolution: FixedPoint::new(72, 0),
        frame_count: 1,
        compressor: Compressor::from(""),
        depth: 0x18,
    };
    let mut body = enc(&visual, "visual sample entry")?;
    match &t.config {
        CodecConfig::Av1 { av1c } => body.extend(config::<Av1c>(av1c, "av1C")?),
        CodecConfig::Avc { avcc } => body.extend(config::<Avcc>(avcc, "avcC")?),
        CodecConfig::Hevc { hvcc } => body.extend(config::<Hvcc>(hvcc, "hvcC")?),
        CodecConfig::Vp9 { vpcc } => body.extend(config::<VpcC>(vpcc, "vpcC")?),
        CodecConfig::Other { .. } => {}
        CodecConfig::Aac { .. } | CodecConfig::Opus { .. } => {
            return Err(invalid(
                "an audio configuration on a video track",
                json!({}),
            ));
        }
    }
    let (p, tr, m, full) = t.colour.to_cicp();
    let nclx = Colr::Nclx {
        colour_primaries: p,
        transfer_characteristics: tr,
        matrix_coefficients: m,
        full_range_flag: full,
    };
    #[cfg(feature = "testing")]
    let colr = match &knobs.colr {
        super::knobs::ColrKnob::Nclx => Some(nclx),
        super::knobs::ColrKnob::Icc(profile) => Some(Colr::Prof {
            profile: profile.clone(),
        }),
        super::knobs::ColrKnob::None => None,
    };
    #[cfg(not(feature = "testing"))]
    let colr = Some(nclx);
    if let Some(colr) = colr {
        body.extend(enc(&colr, "colr")?);
    }
    if t.pixel_aspect.is_positive() && t.pixel_aspect != Rational::from_int(1) {
        let (h, v) = (t.pixel_aspect.num(), t.pixel_aspect.den());
        let pasp = Pasp {
            h_spacing: u32::try_from(h)
                .map_err(|_| invalid("pixel aspect too large", json!({})))?,
            v_spacing: u32::try_from(v)
                .map_err(|_| invalid("pixel aspect too large", json!({})))?,
        };
        body.extend(enc(&pasp, "pasp")?);
    }
    if let Some(hdr) = &t.hdr {
        hdr_boxes(hdr, &mut body);
    }
    Ok(boxed(&t.config.fourcc(), &body))
}

/// The display width written to `tkhd` (before rotation) and the coded height.
pub(crate) fn tkhd_size(t: &VideoTrackConfig) -> (u32, u32) {
    let d = display_size(t.coded, fin_media::Rotation::R0, t.pixel_aspect);
    (d.width, d.height)
}

/// The audio sample entry of `t`.
pub(crate) fn audio_entry(t: &AudioTrackConfig) -> Result<Vec<u8>, CoreError> {
    let audio = Audio {
        data_reference_index: 1,
        channel_count: t.channels,
        sample_size: 16,
        sample_rate: FixedPoint::new(t.sample_rate.min(u16::MAX as u32) as u16, 0),
    };
    let mut body = enc(&audio, "audio sample entry")?;
    match &t.config {
        CodecConfig::Opus { dops } => body.extend(config::<Dops>(dops, "dOps")?),
        CodecConfig::Aac { esds } => body.extend(config::<Esds>(esds, "esds")?),
        CodecConfig::Other { .. } => {}
        _ => {
            return Err(invalid(
                "a video configuration on an audio track",
                json!({}),
            ));
        }
    }
    Ok(boxed(&t.config.fourcc(), &body))
}
