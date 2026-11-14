//! Waveform peaks, `wave` v1 (plan 6C Decision 19): per-channel `i16` min/max pairs at 256,
//! 1,024, 4,096, 16,384 and 65,536 samples per peak, and the `peaks.bin` encoding.

use fin_audio::peaks::{LEVELS, Peaks};
use fin_audio::testing::signals::*;
use fin_audio::{RATE, Stereo, peaks};

const FRAMES: usize = 70_000;

/// A ramp whose sample `i` is `(i − 35000) / 32767`: the peak values are `i − 35000`, clamped.
fn ramp() -> Stereo {
    let l: Vec<f32> = (0..FRAMES)
        .map(|i| (i as i64 - 35_000) as f32 / 32_767.0)
        .collect();
    let r = l.iter().map(|v| -v).collect();
    Stereo { l, r }
}

fn q(k: i64) -> i16 {
    k.clamp(-32_768, 32_767) as i16
}

#[test]
fn peaks_levels_and_header() {
    let p = peaks(&ramp());
    assert_eq!(p.frames, FRAMES as u64);
    assert_eq!(LEVELS, [256, 1_024, 4_096, 16_384, 65_536]);
    assert_eq!(p.levels.len(), 5);
    for (level, spp) in p.levels.iter().zip(LEVELS) {
        assert_eq!(level.samples_per_peak, spp);
        assert_eq!(level.channels.len(), 2);
        let count = FRAMES.div_ceil(spp as usize);
        for (c, sign) in [(0, 1i64), (1, -1i64)] {
            let pairs = &level.channels[c];
            assert_eq!(pairs.len(), count, "level {spp}");
            for (w, &(lo, hi)) in pairs.iter().enumerate() {
                let first = (w * spp as usize) as i64 - 35_000;
                let last = (((w + 1) * spp as usize).min(FRAMES) - 1) as i64 - 35_000;
                let (a, b) = (q(sign * first), q(sign * last));
                assert_eq!(
                    (lo, hi),
                    (a.min(b), a.max(b)),
                    "level {spp} ch {c} peak {w}"
                );
            }
        }
    }
    let bytes = p.to_bytes();
    assert_eq!(&bytes[..8], b"FINWAVE1");
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 5);
    assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 2);
    assert_eq!(
        u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        FRAMES as u64
    );
    assert_eq!(bytes[24..32], [0; 8]);
    let pairs: usize = LEVELS.iter().map(|&s| FRAMES.div_ceil(s as usize)).sum();
    assert_eq!(bytes.len(), 32 + pairs * 2 * 4);
    // The first level's first peak: channel 0 then channel 1, each min then max, little-endian.
    let i16_at = |o: usize| i16::from_le_bytes([bytes[o], bytes[o + 1]]);
    assert_eq!(
        [i16_at(32), i16_at(34), i16_at(36), i16_at(38)],
        [-32_768, -32_768, 32_767, 32_767]
    );
}

#[test]
fn peaks_clamp_and_empty() {
    let s = Stereo {
        l: vec![1.5, -2.0, 0.25],
        r: vec![0.0; 3],
    };
    let p = peaks(&s);
    assert_eq!(p.levels[0].channels[0], vec![(-32_768, 32_767)]);
    assert_eq!(p.levels[4].channels[1], vec![(0, 0)]);
    let e = peaks(&Stereo::default());
    assert_eq!(e.frames, 0);
    assert!(
        e.levels
            .iter()
            .all(|l| l.channels.iter().all(Vec::is_empty))
    );
    assert_eq!(e.to_bytes().len(), 32);
    assert_eq!(Peaks::parse(&e.to_bytes()).unwrap(), e);
}

#[test]
fn peaks_parse_round_trip() {
    let s = Stereo {
        l: pink(1, 3.3, RATE),
        r: noise(2, 3.3, RATE),
    };
    let p = peaks(&s);
    assert_eq!(Peaks::parse(&p.to_bytes()).unwrap(), p);
}

#[test]
fn peaks_parse_refuses_truncated() {
    let b = peaks(&ramp()).to_bytes();
    for len in [0, 7, 31, 32, 33, b.len() - 1] {
        let e = Peaks::parse(&b[..len]).unwrap_err();
        assert_eq!(e.code, "MEDIA_CORRUPT", "length {len}");
    }
    let mut long = b.clone();
    long.push(0);
    assert_eq!(Peaks::parse(&long).unwrap_err().code, "MEDIA_CORRUPT");
    let mut bad = b.clone();
    bad[7] = b'2';
    assert_eq!(Peaks::parse(&bad).unwrap_err().code, "MEDIA_CORRUPT");
    let mut levels = b.clone();
    levels[8] = 4;
    assert_eq!(Peaks::parse(&levels).unwrap_err().code, "MEDIA_CORRUPT");
    let mut chans = b;
    chans[12] = 0;
    assert_eq!(Peaks::parse(&chans).unwrap_err().code, "MEDIA_CORRUPT");
}

/// A pair whose `min` is above its `max` is refused, at the pair's byte offset.
#[test]
fn peaks_parse_refuses_inverted_pair() {
    let mut b = peaks(&ramp()).to_bytes();
    // The second peak of the first level, channel 1: min at 32 + 8 + 4, max at 32 + 8 + 6.
    let at = 32 + 8 + 4;
    b[at..at + 2].copy_from_slice(&100i16.to_le_bytes());
    b[at + 2..at + 4].copy_from_slice(&(-100i16).to_le_bytes());
    let e = Peaks::parse(&b).unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT");
    assert_eq!(e.details["at"], at);
}

/// Pinned: a change here moves an A0 hash and is re-blessed only with an `AUDIO_VERSION` bump.
#[test]
fn peaks_golden_hash() {
    let s = Stereo {
        l: sweep(20.0, 20_000.0, 3.0, RATE),
        r: pink(9, 3.0, RATE),
    };
    let bytes = peaks(&s).to_bytes();
    assert_eq!(fin_audio::AUDIO_VERSION, 1);
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        "f5c467fb0dfdee6f0380f116bf074ea93ea51cae50af40a9f095bc235e3be5ca"
    );
}
