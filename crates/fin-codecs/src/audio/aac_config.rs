//! The AAC configuration records (6C plan Decisions 9 and 12): the AudioSpecificConfig inside an
//! MP4 `esds` (ISO/IEC 14496-1 §7.2.6, 14496-3 §1.6.2.1), read to choose a decoder and to refuse
//! HE-AAC before any decode, and written for the encoder's `esds`. Pure parsing, no `unsafe`.

/// `samplingFrequencyIndex` → Hz (14496-3 Table 1.18).
const RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// AAC-LC's `audioObjectType`.
pub(crate) const AOT_LC: u32 = 2;
/// SBR (HE-AAC) and PS (HE-AAC v2).
const AOT_SBR: u32 = 5;
const AOT_PS: u32 = 29;

/// What an AudioSpecificConfig says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asc {
    /// The core object type (for explicit SBR/PS, the one under the extension).
    pub(crate) object: u32,
    /// The core sampling rate.
    pub(crate) rate: u32,
    /// `channelConfiguration` (0: a program config element defines them).
    pub(crate) channels: u32,
    /// SBR or PS is signalled explicitly (hierarchical object type 5/29, or the backward
    /// compatible sync extension): HE-AAC.
    pub(crate) he_aac: bool,
}

/// A descriptor's (tag, body) at `b[at..]`, and where the next one starts. The length is the
/// 1–4 byte expandable size.
fn descriptor(b: &[u8], at: usize) -> Option<(u8, &[u8], usize)> {
    let tag = *b.get(at)?;
    let mut len = 0usize;
    let mut i = at + 1;
    for _ in 0..4 {
        let byte = *b.get(i)?;
        i += 1;
        len = len << 7 | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            let end = i.checked_add(len)?;
            return Some((tag, b.get(i..end)?, end));
        }
    }
    None
}

/// `esds` box body (version and flags first) → the AudioSpecificConfig bytes, for an MPEG-4 or
/// MPEG-2 AAC object type indication.
pub(crate) fn asc_of_esds(esds: &[u8]) -> Option<&[u8]> {
    let (tag, es, _) = descriptor(esds, 4)?;
    if tag != 0x03 {
        return None;
    }
    let flags = *es.get(2)?;
    let mut at = 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*es.get(at)?);
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    let (tag, dcd, _) = descriptor(es, at)?;
    if tag != 0x04 || !matches!(dcd.first()?, 0x40 | 0x66..=0x68) {
        return None;
    }
    let (tag, asc, _) = descriptor(dcd, 13)?;
    (tag == 0x05).then_some(asc)
}

struct Bits<'a>(&'a [u8], usize);

impl Bits<'_> {
    fn take(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.0.get(self.1 / 8)?;
            v = v << 1 | u32::from(byte >> (7 - self.1 % 8) & 1);
            self.1 += 1;
        }
        Some(v)
    }

    fn left(&self) -> usize {
        (self.0.len() * 8).saturating_sub(self.1)
    }
}

fn object_type(bits: &mut Bits) -> Option<u32> {
    match bits.take(5)? {
        31 => Some(32 + bits.take(6)?),
        o => Some(o),
    }
}

fn frequency(bits: &mut Bits) -> Option<u32> {
    match bits.take(4)? {
        15 => bits.take(24).filter(|r| *r > 0),
        i => RATES.get(i as usize).copied(),
    }
}

/// Parses an AudioSpecificConfig far enough to choose a decoder; `None` when it is cut or
/// malformed.
pub(crate) fn parse_asc(asc: &[u8]) -> Option<Asc> {
    let mut bits = Bits(asc, 0);
    let mut object = object_type(&mut bits)?;
    let rate = frequency(&mut bits)?;
    let channels = bits.take(4)?;
    let mut he_aac = false;
    if matches!(object, AOT_SBR | AOT_PS) {
        he_aac = true;
        frequency(&mut bits)?; // extensionSamplingFrequency
        object = object_type(&mut bits)?;
    }
    // Backward-compatible signalling (14496-3 §1.6.5.2): after an LC GASpecificConfig
    // (frameLengthFlag, dependsOnCoreCoder, extensionFlag), a sync extension 0x2b7 naming SBR.
    if object == AOT_LC && channels != 0 && !he_aac {
        bits.take(1)?;
        if bits.take(1)? == 1 {
            bits.take(14)?;
        }
        bits.take(1)?;
        if bits.left() >= 16 && bits.take(11)? == 0x2b7 {
            let ext = object_type(&mut bits)?;
            if ext == AOT_SBR && bits.take(1)? == 1 {
                he_aac = true;
            }
        }
    }
    Some(Asc {
        object,
        rate,
        channels,
        he_aac,
    })
}

/// The two-byte AudioSpecificConfig of an AAC-LC stream at a table `rate` with `channels` (1 or
/// 2), 1,024-sample frames; `None` for a rate the table lacks.
pub(crate) fn lc_asc(rate: u32, channels: u16) -> Option<[u8; 2]> {
    let index = RATES.iter().position(|r| *r == rate)? as u16;
    let v = (AOT_LC as u16) << 11 | index << 7 | (channels & 0xf) << 3;
    Some(v.to_be_bytes())
}

/// An `esds` box body (version and flags first) carrying `asc`, for an MPEG-4 audio stream of
/// `bitrate` bits a second whose access units are at most `buffer` bytes.
pub(crate) fn esds(asc: &[u8], bitrate: u32, buffer: u32) -> Vec<u8> {
    let sized = |tag: u8, body: &[u8]| {
        // A 4-byte expandable length, as most muxers write it.
        let n = body.len() as u32;
        let mut out = vec![
            tag,
            0x80 | (n >> 21 & 0x7f) as u8,
            0x80 | (n >> 14 & 0x7f) as u8,
            0x80 | (n >> 7 & 0x7f) as u8,
            (n & 0x7f) as u8,
        ];
        out.extend_from_slice(body);
        out
    };
    // objectTypeIndication 0x40 (14496-3 audio), streamType 5 (audio) << 2 | reserved 1.
    let mut dcd = vec![0x40, 0x15];
    dcd.extend_from_slice(&buffer.min(0xff_ffff).to_be_bytes()[1..]);
    dcd.extend_from_slice(&bitrate.to_be_bytes()); // maxBitrate (CBR)
    dcd.extend_from_slice(&bitrate.to_be_bytes()); // avgBitrate
    dcd.extend(sized(0x05, asc));
    // ES_ID 0 (the track's), no stream dependence, URL or OCR; SLConfig predefined 2 (MP4).
    let mut es = vec![0, 0, 0];
    es.extend(sized(0x04, &dcd));
    es.extend(sized(0x06, &[0x02]));
    let mut out = vec![0, 0, 0, 0];
    out.extend(sized(0x03, &es));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lc_round_trips_through_esds() {
        let asc = lc_asc(48_000, 2).unwrap();
        assert_eq!(asc, [0x11, 0x90]);
        let e = esds(&asc, 192_000, 1536);
        assert_eq!(asc_of_esds(&e), Some(&asc[..]));
        let p = parse_asc(&asc).unwrap();
        assert_eq!(
            p,
            Asc {
                object: AOT_LC,
                rate: 48_000,
                channels: 2,
                he_aac: false
            }
        );
        assert_eq!(lc_asc(44_000, 2), None);
    }

    #[test]
    fn explicit_and_backward_compatible_sbr_are_he_aac() {
        // Hierarchical: object 5, core index 6 (24 kHz), stereo, extension index 3, then LC.
        let p = parse_asc(&[0x2B, 0x11, 0x88, 0x00]).unwrap();
        assert!(p.he_aac && p.object == AOT_LC && p.rate == 24_000, "{p:?}");
        // PS (29): 11101 0110 0001 0011 00010 ...
        let p = parse_asc(&[0xEB, 0x09, 0x88, 0x00]).unwrap();
        assert!(p.he_aac, "{p:?}");
        // Backward compatible: LC 24 kHz stereo, GASpecificConfig 000, then 0x2b7, SBR (00101),
        // sbrPresentFlag 1, extension index 3.
        // 00010 0110 0010 000 | 01010110111 | 00101 | 1 | 0011
        let p = parse_asc(&[0x13, 0x10, 0x56, 0xE5, 0x98]).unwrap();
        assert!(p.he_aac && p.object == AOT_LC, "{p:?}");
        // The same with sbrPresentFlag 0 is plain LC.
        let p = parse_asc(&[0x13, 0x10, 0x56, 0xE5, 0x18]).unwrap();
        assert!(!p.he_aac, "{p:?}");
    }

    #[test]
    fn cut_or_foreign_records_are_refused() {
        assert_eq!(parse_asc(&[0x11]), None);
        assert_eq!(parse_asc(&[]), None);
        assert_eq!(asc_of_esds(&[]), None);
        let mut e = esds(&[0x11, 0x90], 128_000, 768);
        let at = e.iter().position(|b| *b == 0x40).unwrap();
        e[at] = 0x21; // H.264 video
        assert_eq!(asc_of_esds(&e), None);
    }
}
