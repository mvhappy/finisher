//! Colour tags (spec §10.1): primaries, transfer, matrix, range and chroma siting, with their
//! ITU-T H.273 (CICP) code points as `colr` `nclx` and codec headers carry them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Primaries {
    Bt709,
    /// BT.601 625-line (BT.470 B/G; PAL).
    #[serde(rename = "bt601_625")]
    Bt601_625,
    /// BT.601 525-line (SMPTE 170M; NTSC).
    #[serde(rename = "bt601_525")]
    Bt601_525,
    Bt2020,
    /// P3 with a D65 white point (SMPTE EG 432-1).
    DisplayP3,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Transfer {
    /// BT.709 (also BT.601 and BT.2020 SDR, whose curves are the same).
    Bt709,
    Srgb,
    /// SMPTE ST 2084.
    Pq,
    /// ARIB STD-B67.
    Hlg,
    Linear,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Matrix {
    Bt709,
    Bt601,
    /// BT.2020 non-constant luminance.
    Bt2020Ncl,
    /// RGB (GBR) coded directly.
    Identity,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Range {
    /// Video range: Y 16–235, chroma 16–240 (8-bit).
    Limited,
    Full,
}

/// Where 4:2:0 chroma samples sit relative to luma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChromaSiting {
    /// Co-sited horizontally with the left luma sample, centred vertically (MPEG-2, H.264
    /// default).
    Left,
    /// Centred both ways (MPEG-1, JPEG).
    Center,
    /// Co-sited with the top-left luma sample (BT.2020, AV1 "colocated").
    TopLeft,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ColourTags {
    pub primaries: Primaries,
    pub transfer: Transfer,
    pub matrix: Matrix,
    pub range: Range,
    pub siting: ChromaSiting,
}

impl Default for ColourTags {
    /// Untagged: every field unknown, limited range.
    fn default() -> Self {
        ColourTags {
            primaries: Primaries::Unknown,
            transfer: Transfer::Unknown,
            matrix: Matrix::Unknown,
            range: Range::Limited,
            siting: ChromaSiting::Unknown,
        }
    }
}

impl ColourTags {
    /// BT.709 throughout, limited range, left siting: what Finisher writes (spec §10.1).
    pub const BT709: ColourTags = ColourTags {
        primaries: Primaries::Bt709,
        transfer: Transfer::Bt709,
        matrix: Matrix::Bt709,
        range: Range::Limited,
        siting: ChromaSiting::Left,
    };

    /// Spec §10.1's untagged rule, field by field: below 720 lines BT.601 (625-line primaries
    /// at 576 and 288 lines, else 525-line), otherwise BT.709; an unknown transfer is BT.709's
    /// curve and an unknown siting is `Left`. Known fields are kept.
    pub fn resolve_defaults(self, height: u32) -> ColourTags {
        let sd = height < 720;
        let primaries = match (self.primaries, sd) {
            (Primaries::Unknown, true) if height == 576 || height == 288 => Primaries::Bt601_625,
            (Primaries::Unknown, true) => Primaries::Bt601_525,
            (Primaries::Unknown, false) => Primaries::Bt709,
            (p, _) => p,
        };
        let matrix = match (self.matrix, sd) {
            (Matrix::Unknown, true) => Matrix::Bt601,
            (Matrix::Unknown, false) => Matrix::Bt709,
            (m, _) => m,
        };
        ColourTags {
            primaries,
            transfer: match self.transfer {
                Transfer::Unknown => Transfer::Bt709,
                t => t,
            },
            matrix,
            range: self.range,
            siting: match self.siting {
                ChromaSiting::Unknown => ChromaSiting::Left,
                s => s,
            },
        }
    }

    /// Tags from H.273 code points (`colr` `nclx`, AV1 and VUI colour descriptions).
    pub fn from_cicp(primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> ColourTags {
        ColourTags {
            primaries: Primaries::from_cicp(primaries),
            transfer: Transfer::from_cicp(transfer),
            matrix: Matrix::from_cicp(matrix),
            range: if full_range {
                Range::Full
            } else {
                Range::Limited
            },
            siting: ChromaSiting::Unknown,
        }
    }

    /// `(primaries, transfer, matrix, full_range)` H.273 code points; unknown is 2
    /// ("unspecified").
    pub fn to_cicp(&self) -> (u16, u16, u16, bool) {
        (
            self.primaries.cicp(),
            self.transfer.cicp(),
            self.matrix.cicp(),
            self.range == Range::Full,
        )
    }
}

impl Primaries {
    pub fn from_cicp(code: u16) -> Primaries {
        match code {
            1 => Primaries::Bt709,
            5 => Primaries::Bt601_625,
            6 | 7 => Primaries::Bt601_525,
            9 => Primaries::Bt2020,
            12 => Primaries::DisplayP3,
            _ => Primaries::Unknown,
        }
    }

    pub fn cicp(self) -> u16 {
        match self {
            Primaries::Bt709 => 1,
            Primaries::Bt601_625 => 5,
            Primaries::Bt601_525 => 6,
            Primaries::Bt2020 => 9,
            Primaries::DisplayP3 => 12,
            Primaries::Unknown => 2,
        }
    }
}

impl Transfer {
    pub fn from_cicp(code: u16) -> Transfer {
        match code {
            1 | 6 | 14 | 15 => Transfer::Bt709,
            13 => Transfer::Srgb,
            16 => Transfer::Pq,
            18 => Transfer::Hlg,
            8 => Transfer::Linear,
            _ => Transfer::Unknown,
        }
    }

    pub fn cicp(self) -> u16 {
        match self {
            Transfer::Bt709 => 1,
            Transfer::Srgb => 13,
            Transfer::Pq => 16,
            Transfer::Hlg => 18,
            Transfer::Linear => 8,
            Transfer::Unknown => 2,
        }
    }
}

impl Matrix {
    pub fn from_cicp(code: u16) -> Matrix {
        match code {
            0 => Matrix::Identity,
            1 => Matrix::Bt709,
            5 | 6 => Matrix::Bt601,
            9 => Matrix::Bt2020Ncl,
            _ => Matrix::Unknown,
        }
    }

    pub fn cicp(self) -> u16 {
        match self {
            Matrix::Identity => 0,
            Matrix::Bt709 => 1,
            Matrix::Bt601 => 6,
            Matrix::Bt2020Ncl => 9,
            Matrix::Unknown => 2,
        }
    }
}
