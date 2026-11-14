//! `DecodedSource` and `CachedSource` (6C plan Task 18): the streamed feed, random reads through
//! `PcmSource`, and a `pcm48` entry's chunks read back equal to the decode.
// Fixtures are written and opened by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

use super::*;
use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_media::cache::{EntryFile, KeyInput};
use fin_media::jobs::{CHUNK_BYTES, chunk_name, pcm_key_input};

/// A stereo (or mono) 16-bit WAV of `frames` frames at `rate`, two different tones.
fn wav_file(dir: &Path, frames: usize, rate: u32, channels: u16) -> PathBuf {
    // The checked open refuses a link at any segment (a temporary directory's `/var`).
    let dir = &std::fs::canonicalize(dir).unwrap();
    let mut pcm = Vec::with_capacity(frames * channels as usize);
    for i in 0..frames {
        let t = i as f64 / rate as f64;
        pcm.push((0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32);
        if channels == 2 {
            pcm.push((0.25 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as f32);
        }
    }
    let path = dir.join("tone.wav");
    std::fs::write(&path, wav(&pcm, channels, rate, WavFormat::Pcm16, None)).unwrap();
    path
}

fn open(path: &Path, recipe: PcmRecipe) -> DecodedSource {
    DecodedSource::open(fin_shm::checked_open(path, u64::MAX).unwrap(), recipe).unwrap()
}

fn stream(mut s: DecodedSource) -> Vec<f32> {
    let mut all = Vec::new();
    while let Some(b) = s.next_interleaved().unwrap() {
        assert!(!b.is_empty());
        all.extend(b);
    }
    all
}

/// `c` as interleaved samples.
fn interleave(c: &Canonical) -> Vec<f32> {
    match c {
        Canonical::Mono(m) => m.clone(),
        Canonical::Stereo(s) => s.l.iter().zip(&s.r).flat_map(|(l, r)| [*l, *r]).collect(),
    }
}

/// Writes `samples` as a PCM entry of `input` into `dir` (what `media.pcm` writes).
fn write_entry(dir: &Path, input: &KeyInput, samples: &[f32]) {
    let mut files = Vec::new();
    for (i, chunk) in samples.chunks(CHUNK_BYTES as usize / 4).enumerate() {
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(dir.join(chunk_name(i)), &bytes).unwrap();
        files.push(EntryFile {
            name: chunk_name(i),
            bytes: bytes.len() as u64,
            blake3: blake3::hash(&bytes).to_hex().to_string(),
        });
    }
    let meta = EntryMeta {
        key: CacheKey::compute(input).to_string(),
        fingerprint: input.fingerprint.clone(),
        recipe: input.recipe.clone(),
        recipe_version: input.recipe_version,
        params: input.params.clone(),
        render_version: input.render_version,
        decoder_class: input.decoder_class,
        encoder: None,
        files,
        media: None,
        tone_mapped: None,
        created_at: 0,
    };
    std::fs::write(dir.join(META_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();
}

#[test]
fn decoded_stream_is_the_resampled_canonical_feed() {
    let tmp = tempfile::tempdir().unwrap();
    // 44.1 kHz: the resampler runs; the length is exactly resampled_len.
    let path = wav_file(tmp.path(), 30_000, 44_100, 2);
    let s = open(&path, PcmRecipe::Pcm48);
    assert_eq!(
        (s.feed_layout(), s.backend()),
        (PcmLayout::Stereo, "symphonia")
    );
    assert_eq!(
        s.expected_frames(),
        Some(fin_audio::resampled_len(30_000, 44_100, 48_000))
    );
    let all = stream(s);
    assert_eq!(
        all.len() as u64,
        2 * fin_audio::resampled_len(30_000, 44_100, 48_000)
    );
    // The 16 kHz feed is mono and folded.
    let m = stream(open(&path, PcmRecipe::Pcm16m));
    assert_eq!(
        m.len() as u64,
        fin_audio::resampled_len(30_000, 44_100, 16_000)
    );
}

#[test]
fn pcm_source_reads_equal_the_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let path = wav_file(tmp.path(), 20_000, 44_100, 2);
    let all = stream(open(&path, PcmRecipe::Pcm48));
    let mut s = open(&path, PcmRecipe::Pcm48);
    assert_eq!(PcmSource::layout(&s), SourceLayout::Stereo);
    let mut c = Canonical::Mono(Vec::new());
    // From before the start to past the end, in uneven blocks: zeros outside.
    let mut got = Vec::new();
    let mut at = -500i64;
    while at < all.len() as i64 / 2 + 700 {
        s.read(at, 777, &mut c).unwrap();
        got.extend(interleave(&c));
        at += 777;
    }
    let mut want = vec![0.0f32; 1000];
    want.extend(&all);
    want.resize(got.len(), 0.0);
    assert_eq!(got, want);
    // A read before what the source still holds is refused, never wrong samples.
    assert_eq!(s.read(0, 10, &mut c).unwrap_err().code, "INTERNAL");
}

#[test]
fn mono_source_keeps_one_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let path = wav_file(tmp.path(), 4_800, 48_000, 1);
    let mut s = open(&path, PcmRecipe::Pcm48);
    assert_eq!(s.feed_layout(), PcmLayout::Mono);
    let mut c = Canonical::Stereo(Stereo::default());
    s.read(0, 4_800, &mut c).unwrap();
    // Identity rate, one channel: the decoded samples themselves, never upmixed.
    let Canonical::Mono(m) = c else {
        panic!("not mono")
    };
    assert_eq!(m.len(), 4_800);
    assert!(
        (m[100] - (0.5 * (2.0 * std::f64::consts::PI * 440.0 * 100.0 / 48_000.0).sin()) as f32)
            .abs()
            < 1e-4
    );
}

#[test]
fn cached_entry_reads_equal_the_decode() {
    let tmp = tempfile::tempdir().unwrap();
    // 25 s at 48 kHz stereo: two chunks (1,048,576 frames, then 151,424).
    let path = wav_file(tmp.path(), 1_200_000, 48_000, 2);
    let all = stream(open(&path, PcmRecipe::Pcm48));
    let entry = std::fs::canonicalize(tmp.path()).unwrap().join("entry");
    std::fs::create_dir(&entry).unwrap();
    let input = pcm_key_input(
        "fp",
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        VendorClass::Software,
    );
    write_entry(&entry, &input, &all);
    let key = CacheKey::compute(&input).to_string();
    let mut cached = CachedSource::open(&entry, &key).unwrap();
    assert_eq!(cached.entry().frames, 1_200_000);
    assert_eq!(cached.entry().chunks.len(), 2);
    let mut decoded = open(&path, PcmRecipe::Pcm48);
    let (mut a, mut b) = (Canonical::Mono(Vec::new()), Canonical::Mono(Vec::new()));
    // Across the chunk boundary and past the end.
    for at in [-10i64, 1_048_500, 1_199_990] {
        cached.read(at, 200, &mut a).unwrap();
        decoded.read(at, 200, &mut b).unwrap();
        assert_eq!(a, b, "at {at}");
    }
    // Another key, or a chunk that is not its digest, is refused.
    let other = CacheKey::compute(&pcm_key_input(
        "fp2",
        PcmRecipe::Pcm48,
        PcmLayout::Stereo,
        VendorClass::Software,
    ));
    assert_eq!(
        CachedSource::open(&entry, other.as_str())
            .err()
            .unwrap()
            .code,
        "PLAN_MISMATCH"
    );
    let mut bytes = std::fs::read(entry.join(chunk_name(1))).unwrap();
    bytes[0] ^= 1;
    std::fs::write(entry.join(chunk_name(1)), bytes).unwrap();
    let mut c = CachedSource::open(&entry, &key).unwrap();
    assert_eq!(
        c.read(1_100_000, 10, &mut a).unwrap_err().code,
        "PLAN_MISMATCH"
    );
}
