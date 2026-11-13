//! The sampling rate of an `esds` box: the AAC AudioSpecificConfig inside its
//! DecoderSpecificInfo (ISO/IEC 14496-1 §7.2.6, 14496-3 §1.6.2.1). Used when the sample entry's
//! 16.16 rate is absent (0: muxers write 0 for rates past 65,535); the track's media timescale
//! is a time unit and says nothing about it.

const RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

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

/// `esds` box body (version and flags first) → the AudioSpecificConfig's sampling rate.
pub(crate) fn sample_rate(esds: &[u8]) -> Option<u32> {
    let (tag, es, _) = descriptor(esds, 4)?;
    if tag != 0x03 {
        return None;
    }
    // ES_ID (2) and flags; the optional dependence, URL and OCR fields precede the config.
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
    if tag != 0x04 {
        return None;
    }
    // objectTypeIndication (0x40 Audio ISO/IEC 14496-3, 0x66–0x68 MPEG-2 AAC), streamType,
    // bufferSizeDB (3), maxBitrate, avgBitrate.
    if !matches!(dcd.first()?, 0x40 | 0x66..=0x68) {
        return None;
    }
    let (tag, asc, _) = descriptor(dcd, 13)?;
    if tag != 0x05 {
        return None;
    }
    asc_rate(asc)
}

/// A sampling frequency index, with the 24-bit explicit rate behind index 15.
fn frequency(bits: &mut Bits) -> Option<u32> {
    match bits.take(4)? {
        15 => bits.take(24).filter(|r| *r > 0),
        i => RATES.get(i as usize).copied(),
    }
}

/// The output rate: the config's own, or for explicitly signalled SBR/PS (object types 5 and
/// 29) the extension rate, which the core `samplingFrequencyIndex` halves.
fn asc_rate(asc: &[u8]) -> Option<u32> {
    let mut bits = Bits(asc, 0);
    let mut object = bits.take(5)?;
    if object == 31 {
        object = 32 + bits.take(6)?;
    }
    let core = frequency(&mut bits)?;
    if matches!(object, 5 | 29) {
        bits.take(4)?; // channelConfiguration
        return frequency(&mut bits).or(Some(core));
    }
    Some(core)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esds(asc: &[u8]) -> Vec<u8> {
        let dsi = [&[0x05, asc.len() as u8][..], asc].concat();
        let mut dcd = vec![0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        dcd.extend(dsi);
        let dcd = [&[0x04, dcd.len() as u8][..], &dcd].concat();
        let es = [&[0u8, 1, 0][..], &dcd, &[0x06, 1, 2]].concat();
        [&[0u8, 0, 0, 0, 0x03, es.len() as u8][..], &es].concat()
    }

    #[test]
    fn reads_the_table_rate() {
        // AAC-LC (2), index 3 (48 kHz), stereo.
        assert_eq!(sample_rate(&esds(&[0x11, 0x90])), Some(48_000));
        // Index 4 (44.1 kHz).
        assert_eq!(sample_rate(&esds(&[0x12, 0x10])), Some(44_100));
    }

    #[test]
    fn reads_an_explicit_rate() {
        // AAC-LC, index 15, 24-bit 96,000 = 0x017700, then channel bits.
        let asc = [0x17, 0x80, 0xBB, 0x80, 0x10];
        assert_eq!(sample_rate(&esds(&asc)), Some(96_000));
    }

    #[test]
    fn explicit_sbr_signals_the_output_rate() {
        // HE-AAC (5), core index 6 (24 kHz), stereo, extension index 3 (48 kHz), then AAC-LC.
        // 00101 0110 0010 0011 00010 -> 0x2B 0x11 0x88 (+ padding).
        assert_eq!(sample_rate(&esds(&[0x2B, 0x11, 0x88, 0x00])), Some(48_000));
        // Without the extension bytes the core rate stands.
        assert_eq!(sample_rate(&esds(&[0x2B, 0x10])), Some(24_000));
    }

    #[test]
    fn refuses_a_non_audio_object_type() {
        let mut e = esds(&[0x11, 0x90]);
        let at = e.iter().position(|b| *b == 0x40).unwrap();
        e[at] = 0x21; // H.264 video
        assert_eq!(sample_rate(&e), None);
    }

    #[test]
    fn refuses_a_reserved_index_and_short_input() {
        assert_eq!(sample_rate(&esds(&[0x17, 0x10])), None); // index 14
        assert_eq!(sample_rate(&esds(&[0x11])), None);
        assert_eq!(sample_rate(&[]), None);
        assert_eq!(sample_rate(&[0, 0, 0, 0, 0x03, 0xff]), None);
    }
}
