//! Colour and HDR boxes of a visual sample entry: `colr` (`nclx`, QuickTime `nclc`, ICC as
//! `prof`/`rICC`), `mdcv`, `clli`, and Dolby Vision `dvcC`/`dvvC`/`dvwC`.

use fin_core::CoreError;
use fin_media::{ColourTags, ContentLight, DolbyVision, MasteringDisplay, Range};
use mp4_atom::Colr;

use super::reader::{BoxHeader, be_u16, be_u32, decode};
use crate::error::corrupt;

/// What a `colr` box says: code points, or that an ICC profile was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColrInfo {
    Tags(ColourTags),
    Icc,
}

pub(crate) fn colr(body: &[u8], h: &BoxHeader) -> Result<Option<ColrInfo>, CoreError> {
    // An unknown colour type is not an error: the entry is read as untagged.
    if body.len() < 4 || !matches!(&body[..4], b"nclx" | b"nclc" | b"prof" | b"rICC") {
        return Ok(None);
    }
    Ok(Some(match decode::<Colr>(body, h)? {
        Colr::Nclx {
            colour_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range_flag,
        } => ColrInfo::Tags(ColourTags::from_cicp(
            colour_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range_flag,
        )),
        Colr::Nclc {
            colour_primaries,
            transfer_characteristics,
            matrix_coefficients,
        } => {
            let mut t = ColourTags::from_cicp(
                colour_primaries,
                transfer_characteristics,
                matrix_coefficients,
                false,
            );
            t.range = Range::Limited;
            ColrInfo::Tags(t)
        }
        _ => ColrInfo::Icc,
    }))
}

pub(crate) fn mdcv(body: &[u8], h: &BoxHeader) -> Result<MasteringDisplay, CoreError> {
    let short = || corrupt("the mdcv box is cut short", h.start, "mdcv");
    let u = |at| be_u16(body, at).ok_or_else(short);
    Ok(MasteringDisplay {
        primaries: [[u(0)?, u(2)?], [u(4)?, u(6)?], [u(8)?, u(10)?]],
        white_point: [u(12)?, u(14)?],
        max_luminance: be_u32(body, 16).ok_or_else(short)?,
        min_luminance: be_u32(body, 20).ok_or_else(short)?,
    })
}

pub(crate) fn clli(body: &[u8], h: &BoxHeader) -> Result<ContentLight, CoreError> {
    let short = || corrupt("the clli box is cut short", h.start, "clli");
    Ok(ContentLight {
        max_cll: be_u16(body, 0).ok_or_else(short)?,
        max_fall: be_u16(body, 2).ok_or_else(short)?,
    })
}

/// A Dolby Vision decoder configuration record.
pub(crate) fn dovi(body: &[u8], h: &BoxHeader) -> Result<DolbyVision, CoreError> {
    if body.len() < 5 {
        return Err(corrupt(
            "the Dolby Vision configuration is cut short",
            h.start,
            &h.name(),
        ));
    }
    Ok(DolbyVision {
        profile: body[2] >> 1,
        level: ((body[2] & 1) << 5) | (body[3] >> 3),
        rpu: body[3] & 0b100 != 0,
        enhancement_layer: body[3] & 0b10 != 0,
        base_layer: body[3] & 1 != 0,
        compatibility: body[4] >> 4,
    })
}
