//! Review #24: `audio.sampleRate` is the audio's sample rate (sample entry, AAC
//! AudioSpecificConfig), never the track's media timescale.

use std::path::Path;

use fin_probe::fixtures::{FixtureOpts, write_m4a};
use fin_probe::probe;

fn find(bytes: &[u8], kind: &[u8; 4]) -> usize {
    bytes
        .windows(4)
        .position(|w| w == kind)
        .unwrap_or_else(|| panic!("no {} box", String::from_utf8_lossy(kind)))
}

fn put_u32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A 48 kHz M4A whose media timescale is 1000 (durations scaled): the layout encoders that
/// keep a millisecond track clock produce. `entry_rate` replaces the sample entry's 16.16 rate.
fn rescaled(path: &Path, entry_rate: Option<u32>) {
    let opts = FixtureOpts {
        seconds: 3,
        ..FixtureOpts::default()
    };
    write_m4a(path, opts);
    let mut b = std::fs::read(path).unwrap();
    // mdhd version 1: version/flags, created (8), modified (8), timescale, duration (8).
    let mdhd = find(&b, b"mdhd") + 4;
    assert_eq!(b[mdhd], 1, "the muxer writes version 1");
    put_u32(&mut b, mdhd + 20, 1000);
    b[mdhd + 24..mdhd + 32].copy_from_slice(&(3 * 1000u64).to_be_bytes());
    // stts: version/flags, count, then (count, delta) pairs; one sample per second.
    let stts = find(&b, b"stts") + 4;
    assert_eq!(&b[stts + 4..stts + 8], &1u32.to_be_bytes());
    put_u32(&mut b, stts + 8, 3);
    put_u32(&mut b, stts + 12, 1000);
    // An edit list, if any, counts movie ticks and so stays as written.
    if let Some(rate) = entry_rate {
        // mp4a: 6 reserved, data ref index, 8 reserved, channels, size, 4, rate (16.16).
        let mp4a = find(&b, b"mp4a") + 4;
        put_u32(&mut b, mp4a + 24, rate);
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn rate_is_not_the_media_timescale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ms.m4a");
    rescaled(&path, None);
    let p = probe(&path).unwrap();
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    let m = p.media.unwrap().audio.unwrap();
    assert_eq!(m.sample_rate, 48_000);
    assert_eq!(p.duration.map(|d| d.to_string()).as_deref(), Some("3"));
}

#[test]
fn rate_comes_from_the_audio_specific_config_when_the_entry_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ms0.m4a");
    rescaled(&path, Some(0));
    let a = probe(&path).unwrap().audio.unwrap();
    assert_eq!(a.sample_rate, 48_000);
}

#[test]
fn entry_rate_wins_over_a_conflicting_timescale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.m4a");
    rescaled(&path, Some(44_100 << 16));
    // The esds says 48,000, the entry 44,100: the container's own sample entry is the rate
    // of record unless it is absent or cannot hold the value.
    let a = probe(&path).unwrap().audio.unwrap();
    assert_eq!(a.sample_rate, 44_100);
}
