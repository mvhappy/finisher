//! HDR to SDR (spec §10.1, Windows and Linux; macOS frames arrive tone-mapped by the OS):
//! PQ through the ST 2084 EOTF to absolute luminance, HLG through the inverse OETF and the
//! BT.2100 OOTF at a 1,000-nit nominal peak to display light; then the BT.2390 EETF on the
//! PQ-encoded `max(R, G, B)` (source peak from `clli`/`mdcv` or 1,000 nits, a 100-nit SDR
//! target, black 0), its ratio applied to all three channels (no hue shift); then the
//! primaries conversion to Rec.709, the soft clip and the BT.1886 encode.
//!
//! [`to_sdr`] is the one definition: the CPU reference calls it through
//! [`Conversion::apply`], and `fin-compositor`'s `shaders/tonemap.wgsl` is its line-for-line
//! port (T1 holds between them). The constants that depend on the source ([`Eetf`]) are
//! computed here once and handed to both. Scalar `f32`, `libm` only, in a fixed order (spec
//! §13.4). Zero inputs to `powf` are answered before the call, as the shader must (`pow(0, y)`
//! is not defined on every GPU).

use super::convert::{convert_gamut, linear_to_709, primaries_to_709};
use super::{ColourTags, Transfer};
use crate::info::HdrInfo;

const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;
/// PQ's absolute peak, cd/m².
const PQ_PEAK: f32 = 10_000.0;

const HLG_A: f32 = 0.178_832_77;
const HLG_B: f32 = 0.284_668_92;
const HLG_C: f32 = 0.559_910_7; // 0.559 910 73 (BT.2100), the same f32
/// HLG's nominal display peak for the OOTF (BT.2100 reference, spec §10.1), cd/m².
pub const HLG_PEAK: f32 = 1000.0;
/// The SDR target's peak, cd/m².
pub const SDR_PEAK: f32 = 100.0;
/// A PQ source's peak when it carries neither `clli` nor `mdcv`, cd/m².
pub const DEFAULT_PEAK: f32 = 1000.0;

/// The BT.2020 luminance weights the HLG OOTF uses.
const YS: [f32; 3] = [0.2627, 0.6780, 0.0593];
const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// ST 2084 EOTF: a PQ signal (clamped to 0…1) to absolute luminance in cd/m².
pub fn pq_eotf(e: f32) -> f32 {
    let e = e.clamp(0.0, 1.0);
    if e <= 0.0 {
        return 0.0;
    }
    let p = libm::powf(e, 1.0 / PQ_M2);
    let num = (p - PQ_C1).max(0.0);
    if num <= 0.0 {
        return 0.0;
    }
    let den = PQ_C2 - PQ_C3 * p;
    PQ_PEAK * libm::powf(num / den, 1.0 / PQ_M1)
}

/// The ST 2084 inverse EOTF: luminance in cd/m² (clamped to 0…10 000) to a PQ signal.
pub fn pq_inverse_eotf(nits: f32) -> f32 {
    let y = if nits > 0.0 {
        libm::powf(nits.clamp(0.0, PQ_PEAK) / PQ_PEAK, PQ_M1)
    } else {
        0.0
    };
    libm::powf((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2)
}

/// The BT.2100 HLG inverse OETF: a signal (clamped to 0…1) to scene-linear light in 0…1.
pub fn hlg_inverse_oetf(e: f32) -> f32 {
    let e = e.clamp(0.0, 1.0);
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (libm::expf((e - HLG_C) / HLG_A) + HLG_B) / 12.0
    }
}

/// The BT.2100 HLG OOTF at [`HLG_PEAK`] (γ 1.2, black 0): scene-linear RGB to display light in
/// cd/m², `Fd = Lw · Ys^(γ − 1) · E`.
pub fn hlg_ootf(scene: [f32; 3]) -> [f32; 3] {
    let ys = YS[0] * scene[0] + YS[1] * scene[1] + YS[2] * scene[2];
    if ys <= 0.0 {
        return [0.0; 3];
    }
    let gain = HLG_PEAK * libm::powf(ys, 0.2);
    [gain * scene[0], gain * scene[1], gain * scene[2]]
}

/// The BT.2390 EETF's constants for one source peak and target (black 0 for both): the
/// PQ-encoded source black and range, the normalised target maximum and minimum, and the knee
/// `KS = 1.5 · maxLum − 0.5`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eetf {
    /// `PQ(source black)`.
    pub black: f32,
    /// `PQ(source peak) − PQ(source black)`.
    pub range: f32,
    /// The target peak, normalised: `(PQ(target peak) − black) / range`.
    pub max_lum: f32,
    /// The target black, normalised (0: SDR black is 0).
    pub min_lum: f32,
    /// Where the Hermite roll-off starts; ≥ 1 means none (the source fits the target).
    pub knee: f32,
}

impl Eetf {
    pub fn new(source_peak: f32, target_peak: f32) -> Eetf {
        let black = pq_inverse_eotf(0.0);
        let range = pq_inverse_eotf(source_peak) - black;
        let max_lum = (pq_inverse_eotf(target_peak) - black) / range;
        let min_lum = (pq_inverse_eotf(0.0) - black) / range;
        Eetf {
            black,
            range,
            max_lum,
            min_lum,
            knee: 1.5 * max_lum - 0.5,
        }
    }

    /// BT.2390 §5.4 on a PQ signal: normalised to the source range (clamped to 0…1), the
    /// Hermite spline above the knee, the black-level term, back to a PQ signal.
    pub fn apply_pq(&self, e: f32) -> f32 {
        let e1 = ((e - self.black) / self.range).clamp(0.0, 1.0);
        let e2 = if self.knee < 1.0 && e1 >= self.knee {
            let t = (e1 - self.knee) / (1.0 - self.knee);
            let t2 = t * t;
            let t3 = t2 * t;
            (2.0 * t3 - 3.0 * t2 + 1.0) * self.knee
                + (t3 - 2.0 * t2 + t) * (1.0 - self.knee)
                + (-2.0 * t3 + 3.0 * t2) * self.max_lum
        } else {
            e1
        };
        let r = 1.0 - e2;
        let e3 = e2 + self.min_lum * (r * r * r * r);
        e3 * self.range + self.black
    }

    /// The EETF on luminance in cd/m².
    pub fn apply(&self, nits: f32) -> f32 {
        pq_eotf(self.apply_pq(pq_inverse_eotf(nits)))
    }
}

/// What [`to_sdr`] needs: the EETF, the target peak and the linear-light primaries
/// conversion to Rec.709 (the identity for Rec.709 primaries).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneMap {
    pub eetf: Eetf,
    pub target_peak: f32,
    pub gamut: [[f32; 3]; 3],
}

/// Display light in cd/m² (the source's primaries) to display-encoded Rec.709 SDR: the EETF on
/// `max(R, G, B)` and its ratio on every channel, relative to the target peak, the primaries
/// matrix, the soft clip, the BT.1886 encode.
pub fn to_sdr(rgb: [f32; 3], p: &ToneMap) -> [f32; 3] {
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if m <= 0.0 {
        return [0.0; 3];
    }
    let s = p.eetf.apply(m) / m / p.target_peak;
    linear_to_709([rgb[0] * s, rgb[1] * s, rgb[2] * s], &p.gamut)
}

/// The source's peak luminance for the EETF: HLG's nominal [`HLG_PEAK`]; for PQ, `clli`'s
/// MaxCLL when set, else `mdcv`'s maximum, else [`DEFAULT_PEAK`], kept within
/// [`SDR_PEAK`]…10 000.
pub fn source_peak(transfer: Transfer, hdr: Option<&HdrInfo>) -> f32 {
    if transfer == Transfer::Hlg {
        return HLG_PEAK;
    }
    let cll = hdr
        .and_then(|h| h.content_light)
        .map(|c| f32::from(c.max_cll))
        .filter(|&v| v > 0.0);
    let mdcv = hdr
        .and_then(|h| h.mastering)
        .map(|m| m.max_luminance as f32 / 10_000.0)
        .filter(|&v| v > 0.0);
    cll.or(mdcv)
        .unwrap_or(DEFAULT_PEAK)
        .clamp(SDR_PEAK, PQ_PEAK)
}

/// The HDR transfers that are tone-mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hdr {
    Pq,
    Hlg,
}

/// How a source's display-encoded R′G′B′ reaches the Rec.709 SDR working space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conversion {
    /// Rec.709 (and the BT.601 primaries, treated as Rec.709) SDR: as decoded.
    None,
    /// Wide-gamut SDR (Display P3, BT.2020): BT.1886-linearised, the 3×3 matrix, the soft
    /// clip, re-encoded.
    Gamut([[f32; 3]; 3]),
    /// PQ or HLG: tone-mapped by [`to_sdr`].
    Hdr(Hdr, ToneMap),
}

impl Conversion {
    /// The conversion for resolved tags and the source's HDR metadata.
    pub fn for_source(tags: &ColourTags, hdr: Option<&HdrInfo>) -> Conversion {
        let gamut = primaries_to_709(tags.primaries);
        let kind = match tags.transfer {
            Transfer::Pq => Hdr::Pq,
            Transfer::Hlg => Hdr::Hlg,
            _ => return gamut.map_or(Conversion::None, Conversion::Gamut),
        };
        Conversion::Hdr(
            kind,
            ToneMap {
                eetf: Eetf::new(source_peak(tags.transfer, hdr), SDR_PEAK),
                target_peak: SDR_PEAK,
                gamut: gamut.unwrap_or(IDENTITY),
            },
        )
    }

    /// One display-encoded R′G′B′ pixel to display-encoded Rec.709 SDR.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        match self {
            Conversion::None => rgb,
            Conversion::Gamut(m) => convert_gamut(rgb, m),
            Conversion::Hdr(Hdr::Pq, p) => {
                to_sdr([pq_eotf(rgb[0]), pq_eotf(rgb[1]), pq_eotf(rgb[2])], p)
            }
            Conversion::Hdr(Hdr::Hlg, p) => {
                let scene = [
                    hlg_inverse_oetf(rgb[0]),
                    hlg_inverse_oetf(rgb[1]),
                    hlg_inverse_oetf(rgb[2]),
                ];
                to_sdr(hlg_ootf(scene), p)
            }
        }
    }
}
