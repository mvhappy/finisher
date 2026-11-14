//! Audio decode through `AudioReader` (6C plan Task 12): symphonia's containers and codecs at
//! the source's own rate and channel layout, never resampled or downmixed. Fixtures are written
//! at test time by `fin_codecs::audio::testing`; MP3, Vorbis and ALAC audio, which nothing in the
//! tree encodes, come from `FIN_MEDIA_FIXTURES` and are skipped with a notice without it.

use std::io::Cursor;

use fin_audio::testing::{analyse, signals};
use fin_codecs::audio::testing::{aiff, caf, flac, mp3, ogg, quantised, wav};
use fin_codecs::{AudioLimits, AudioReader};
use fin_media::AudioCodec;
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use fin_media::backend::VendorClass;

/// Every frame of `bytes`: the reader, its samples (interleaved) and the layout of the chunks.
fn decode(bytes: Vec<u8>) -> Result<(AudioReader, Vec<f32>, ChannelLayout), fin_core::CoreError> {
    let mut r = AudioReader::open(Cursor::new(bytes), &AudioLimits::default())?;
    let mut pcm = Vec::new();
    let mut layout = ChannelLayout::default();
    let mut next_pts = 0i64;
    while let Some(chunk) = r.next()? {
        assert_eq!(chunk.rate, r.info().sample_rate);
        assert_eq!(chunk.pts, next_pts, "chunks are contiguous from sample 0");
        assert_eq!(chunk.interleaved.len() % chunk.layout.channels(), 0);
        next_pts += (chunk.interleaved.len() / chunk.layout.channels()) as i64;
        layout = chunk.layout;
        pcm.extend_from_slice(&chunk.interleaved);
    }
    Ok((r, pcm, layout))
}

/// A stereo test signal: a 997 Hz sine at −6 dBFS left, a 440 Hz sine at −12 dBFS right.
fn stereo(secs: f64, rate: u32) -> Vec<f32> {
    let l = signals::sine(997.0, -6.0, secs, rate);
    let r = signals::sine(440.0, -12.0, secs, rate);
    l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect()
}

fn bits_eq(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn wav_pcm16_24_32f_bit_exact() {
    let src = stereo(0.5, 48_000);
    for (format, want) in [
        (wav::WavFormat::Pcm16, quantised(&src, 16)),
        (wav::WavFormat::Pcm24, quantised(&src, 24)),
        (wav::WavFormat::F32, src.clone()),
    ] {
        let (r, pcm, layout) = decode(wav::wav(&src, 2, 48_000, format, None)).unwrap();
        assert_eq!(r.info().codec, AudioCodec::Pcm, "{format:?}");
        assert_eq!(layout, ChannelLayout(vec![ChannelPos::L, ChannelPos::R]));
        assert!(bits_eq(&pcm, &want), "{format:?} decoded bit-exactly");
    }
}

#[test]
fn aiff_and_caf_lpcm_bit_exact() {
    let src = stereo(0.25, 48_000);
    for bits in [16u16, 24] {
        let (r, pcm, _) = decode(aiff::aiff(&src, 2, 48_000, bits)).unwrap();
        assert_eq!(r.info().codec, AudioCodec::Pcm);
        assert!(bits_eq(&pcm, &quantised(&src, bits as u32)), "AIFF {bits}");
    }
    for (format, want) in [
        (caf::CafFormat::I16Be, quantised(&src, 16)),
        (caf::CafFormat::F32Le, src.clone()),
    ] {
        let (r, pcm, _) = decode(caf::caf(&src, 2, 48_000, format)).unwrap();
        assert_eq!(r.info().codec, AudioCodec::Pcm);
        assert!(bits_eq(&pcm, &want), "CAF {format:?}");
    }
}

#[test]
fn flac_verbatim_bit_exact() {
    let src = stereo(0.3, 44_100);
    for bits in [16u32, 24] {
        // 4,096-frame blocks; 13,230 frames end on a short block.
        let (r, pcm, _) = decode(flac::flac(&src, 2, 44_100, bits, 4096)).unwrap();
        assert_eq!(r.info().codec, AudioCodec::Flac);
        assert_eq!(r.info().sample_rate, 44_100);
        assert!(bits_eq(&pcm, &quantised(&src, bits)), "FLAC {bits}");
    }
}

#[test]
fn wav_layout_from_mask() {
    let frames = 4_800;
    let src: Vec<f32> = (0..frames * 6)
        .map(|i| ((i % 6) as f32 - 2.5) / 8.0)
        .collect();
    let (r, pcm, layout) = decode(wav::wav(
        &src,
        6,
        48_000,
        wav::WavFormat::Pcm16,
        Some(0x60F),
    ))
    .unwrap();
    use ChannelPos::*;
    assert_eq!(layout, ChannelLayout(vec![L, R, C, Lfe, Ls, Rs]));
    assert_eq!(r.info().channels, 6);
    // The decoder's own order: nothing is reordered or downmixed.
    assert!(bits_eq(&pcm, &quantised(&src, 16)));
    // Without a mask, the WAVE default for the count (Decision 3), not symphonia's guess.
    let (_, _, quad) = decode(wav::wav(
        &src[..4 * 100],
        4,
        48_000,
        wav::WavFormat::Pcm16,
        None,
    ))
    .unwrap();
    assert_eq!(quad, ChannelLayout::wave_default(4));
    let (_, _, zero) = decode(wav::wav(&src, 6, 48_000, wav::WavFormat::Pcm16, Some(0))).unwrap();
    assert_eq!(zero, ChannelLayout::wave_default(6));
}

#[test]
fn wav_44100_native_rate() {
    let src = signals::sine(1_000.0, -3.0, 1.0, 44_100);
    let (r, pcm, _) = decode(wav::wav(&src, 1, 44_100, wav::WavFormat::Pcm16, None)).unwrap();
    assert_eq!(r.info().sample_rate, 44_100);
    assert_eq!(pcm.len(), 44_100, "frames exact at the source rate");
    assert_eq!(r.declared_frames(), Some(44_100));
}

#[test]
fn mono_wav_reports_mono() {
    let src = signals::sine(1_000.0, -3.0, 0.1, 48_000);
    let (r, pcm, layout) = decode(wav::wav(&src, 1, 48_000, wav::WavFormat::F32, None)).unwrap();
    assert_eq!(r.info().channels, 1);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::C]));
    assert!(bits_eq(&pcm, &src));
    let canonical = fin_audio::channels::canonical(&pcm, &layout).unwrap();
    assert!(matches!(canonical, fin_audio::channels::Canonical::Mono(_)));
}

#[test]
fn vorbis_codec_mapped() {
    let (r, pcm, layout) = decode(ogg::vorbis_silence(2, 48_000, 40)).unwrap();
    assert_eq!(r.info().codec, AudioCodec::Vorbis);
    assert_eq!(r.info().sample_rate, 48_000);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::L, ChannelPos::R]));
    assert_eq!(pcm.len(), 39 * 128 * 2);
    assert!(pcm.iter().all(|&x| x == 0.0));
}

#[test]
fn truncated_files_typed() {
    let src = stereo(0.2, 48_000);
    let files = [
        (
            "wav16",
            wav::wav(&src, 2, 48_000, wav::WavFormat::Pcm16, None),
        ),
        (
            "wav24",
            wav::wav(&src, 2, 48_000, wav::WavFormat::Pcm24, Some(0x3)),
        ),
        (
            "wavf32",
            wav::wav(&src, 2, 48_000, wav::WavFormat::F32, None),
        ),
        ("aiff", aiff::aiff(&src, 2, 48_000, 16)),
        ("caf", caf::caf(&src, 2, 48_000, caf::CafFormat::I16Be)),
        ("flac", flac::flac(&src, 2, 48_000, 16, 1152)),
        ("ogg", ogg::vorbis_silence(2, 48_000, 60)),
    ];
    for (name, bytes) in files {
        let len = bytes.len();
        // Inside the headers, then across the payload.
        let cuts = [
            4,
            12,
            24,
            40,
            64,
            100,
            len / 4,
            len / 2,
            3 * len / 4,
            len - 1,
        ];
        for cut in cuts {
            let got = std::panic::catch_unwind(|| decode(bytes[..cut].to_vec()).map(|_| ()))
                .unwrap_or_else(|_| panic!("{name} cut at {cut} panicked"));
            let e = got.expect_err(&format!("{name} cut at {cut} of {len} is an error"));
            assert!(
                e.code == "MEDIA_CORRUPT" || e.code == "DECODE_FAILED",
                "{name} cut at {cut}: {} {}",
                e.code,
                e.message
            );
        }
        // The whole file decodes.
        decode(bytes).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

#[test]
fn corrupt_bytes_never_panic() {
    let src = stereo(0.1, 48_000);
    let files = [
        (
            "wav",
            wav::wav(&src, 2, 48_000, wav::WavFormat::Pcm24, Some(0x3)),
        ),
        ("aiff", aiff::aiff(&src, 2, 48_000, 24)),
        ("caf", caf::caf(&src, 2, 48_000, caf::CafFormat::F32Le)),
        ("flac", flac::flac(&src, 2, 48_000, 24, 576)),
        ("ogg", ogg::vorbis_silence(2, 48_000, 30)),
    ];
    let typed = [
        "MEDIA_CORRUPT",
        "DECODE_FAILED",
        "CODEC_UNAVAILABLE",
        "MEDIA_LIMIT",
        "UNSUPPORTED_CONTAINER",
    ];
    for (name, bytes) in files {
        for k in 0..40 {
            let mut b = bytes.clone();
            // Hit the headers densely, then the payload.
            let at = if k < 20 {
                k * 3
            } else {
                (b.len() - 1) * (k - 20) / 20
            };
            b[at] ^= 0xA5;
            let got = std::panic::catch_unwind(|| decode(b).map(|_| ()))
                .unwrap_or_else(|_| panic!("{name} with byte {at} flipped panicked"));
            if let Err(e) = got {
                assert!(typed.contains(&e.code.as_str()), "{name} byte {at}: {e:?}");
            }
        }
    }
}

#[test]
fn decoder_class_is_software() {
    let src = stereo(0.05, 48_000);
    let r = AudioReader::open(
        Cursor::new(wav::wav(&src, 2, 48_000, wav::WavFormat::Pcm16, None)),
        &AudioLimits::default(),
    )
    .unwrap();
    assert_eq!(r.decoder_class(), VendorClass::Software);
    assert_eq!(r.backend().0, "symphonia");
}

#[test]
fn reads_are_bounded() {
    // 9 channels and 768 kHz are past the reader's bounds: MEDIA_LIMIT before decoding.
    let nine = vec![0.0f32; 9 * 16];
    let e = decode(wav::wav(&nine, 9, 48_000, wav::WavFormat::Pcm16, None)).unwrap_err();
    assert_eq!(e.code, "MEDIA_LIMIT", "{e:?}");
    let e = decode(wav::wav(
        &[0.0; 16],
        1,
        768_000,
        wav::WavFormat::Pcm16,
        None,
    ))
    .unwrap_err();
    assert_eq!(e.code, "MEDIA_LIMIT", "{e:?}");
    // A declared length past 24 h (a FLAC STREAMINFO saying 25 h at 8 kHz).
    let mut f = flac::flac(&[0.0; 64], 1, 8_000, 16, 64);
    let info = flac::streaminfo(1, 8_000, 16, 64, 25 * 3600 * 8_000);
    f[8..42].copy_from_slice(&info);
    let e = decode(f).unwrap_err();
    assert_eq!(e.code, "MEDIA_LIMIT", "{e:?}");
}

#[test]
fn not_audio_is_unsupported() {
    let e = decode(b"definitely not a media file".to_vec()).unwrap_err();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER", "{e:?}");
}

/// One fixture-cache row: `<FIN_MEDIA_FIXTURES>/audio/<file>` with `frames` (at the source rate)
/// and `rmsDbfs` (over all channels) recorded in `audio/manifest.json`.
#[test]
fn mp3_vorbis_alac_from_fixture_cache() {
    let Some(dir) = std::env::var_os("FIN_MEDIA_FIXTURES") else {
        if std::env::var("FIN_REQUIRE_MEDIA").as_deref() == Ok("1") {
            panic!("FIN_REQUIRE_MEDIA=1 but FIN_MEDIA_FIXTURES is not set");
        }
        eprintln!("notice: FIN_MEDIA_FIXTURES is not set; MP3, Vorbis and ALAC decode skipped");
        return;
    };
    let dir = std::path::PathBuf::from(dir).join("audio");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest.json"))
            .expect("manifest.json is JSON");
    // (file, codec, one codec frame in samples)
    for (file, codec, frame) in [
        ("mp3.mp3", AudioCodec::Mp3, 1_152u64),
        ("vorbis.ogg", AudioCodec::Vorbis, 2_048),
        ("alac.caf", AudioCodec::Alac, 4_096),
    ] {
        let row = &manifest[file];
        let (r, pcm, layout) =
            decode(std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}")))
                .unwrap_or_else(|e| panic!("{file}: {e:?}"));
        assert_eq!(r.info().codec, codec, "{file}");
        let frames = (pcm.len() / layout.channels()) as u64;
        let want = row["frames"].as_u64().expect("frames");
        assert!(
            frames.abs_diff(want) <= frame,
            "{file}: {frames} frames, recorded {want}"
        );
        let rms = analyse::rms_dbfs(&pcm);
        let want = row["rmsDbfs"].as_f64().expect("rmsDbfs");
        assert!(
            (rms - want).abs() <= 0.5,
            "{file}: {rms:.2} dBFS, recorded {want:.2}"
        );
    }
}

/// An MP4 with one audio track of `config`, 10 packets of 1,152 samples.
fn mp4_audio(config: fin_container::CodecConfig, priming: u32) -> Vec<u8> {
    let mut w = fin_container::Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let t = w
        .add_audio(fin_container::AudioTrackConfig {
            config,
            sample_rate: 48_000,
            channels: 2,
            priming,
        })
        .unwrap();
    for i in 0..10i64 {
        w.write(t, &[0x55; 200], i * 1152, i * 1152, 1152, true)
            .unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn mp4_audio_routes_by_codec() {
    use fin_container::CodecConfig;
    // Opus and AAC arrive with Tasks 13–16.
    let dops = vec![0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 0];
    let e = decode(mp4_audio(CodecConfig::Opus { dops }, 312)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["codec"]),
        ("CODEC_UNAVAILABLE", &"opus".into())
    );
    // ALAC in MP4 needs its `alac` box, which the demuxer does not hand out.
    let e = decode(mp4_audio(CodecConfig::Other { fourcc: *b"alac" }, 0)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("CODEC_UNAVAILABLE", &"config".into())
    );
    // MP3 in MP4 opens symphonia's decoder from the track's rate and channels; these packets
    // are not MPEG audio, so the decode fails typed.
    let mut r = AudioReader::open(
        Cursor::new(mp4_audio(CodecConfig::Other { fourcc: *b".mp3" }, 0)),
        &AudioLimits::default(),
    )
    .unwrap();
    assert_eq!(
        (r.info().codec, r.backend().0),
        (AudioCodec::Mp3, "symphonia")
    );
    loop {
        match r.next() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                assert!(
                    ["DECODE_FAILED", "MEDIA_CORRUPT"].contains(&e.code.as_str()),
                    "{e:?}"
                );
                break;
            }
        }
    }
}

/// An MP4 of 12 silent stereo MP3 frames, priming 1.
fn mp4_mp3() -> Vec<u8> {
    let mut w = fin_container::Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let t = w
        .add_audio(fin_container::AudioTrackConfig {
            config: fin_container::CodecConfig::Other { fourcc: *b".mp3" },
            sample_rate: 48_000,
            channels: 2,
            priming: 1,
        })
        .unwrap();
    for i in 0..12i64 {
        let n = mp3::FRAME_SAMPLES as i64;
        w.write(t, &mp3::frame(2), i * n, i * n, n as u32, true)
            .unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn mp4_hostile_padding_is_corrupt() {
    let bytes = mp4_mp3();
    let (r, pcm, _) = decode(bytes.clone()).unwrap();
    assert_eq!(r.info().codec, AudioCodec::Mp3);
    assert_eq!(pcm.len() / 2, 12 * 1152 - 1, "priming 1 trimmed");
    // Every `stts` delta set to u32::MAX: the edit list then leaves almost all of it as padding,
    // which must not be held in memory (nor pass the decode budget).
    let at = bytes.windows(4).position(|w| w == b"stts").expect("stts");
    let mut bad = bytes.clone();
    let entries = u32::from_be_bytes(bad[at + 8..at + 12].try_into().unwrap()) as usize;
    for e in 0..entries {
        let d = at + 12 + e * 8 + 4;
        bad[d..d + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    }
    let e = decode(bad).unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
    assert_eq!(e.details["at"], "edts");
}

#[test]
fn wav_layout_after_a_large_chunk() {
    let src = vec![0.25f32; 4 * 64];
    let plain = wav::wav(&src, 4, 48_000, wav::WavFormat::Pcm16, None);
    let (_, _, layout) = decode(wav::with_junk(&plain, 70_000, 70_000)).unwrap();
    assert_eq!(
        layout,
        ChannelLayout::wave_default(4),
        "fmt after 70,000 bytes of JUNK"
    );
    // A chunk claiming more bytes than the file has, before `fmt `, is corrupt.
    let e = decode(wav::with_junk(&plain, 16, u32::MAX - 1)).unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
}

#[test]
fn mp3_xing_length_is_exact() {
    let full = mp3::mp3(1, 40, true);
    let (r, pcm, _) = decode(full.clone()).unwrap();
    assert_eq!(r.info().codec, AudioCodec::Mp3);
    assert_eq!(
        r.declared_frames(),
        Some(pcm.len() as u64),
        "the Xing count is met"
    );
    // One byte short of the stated frames: corrupt, not short.
    let e = decode(full[..full.len() - 1].to_vec()).unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
    // Without a Xing tag the length is an estimate: a cut file still decodes what is there.
    let plain = mp3::mp3(1, 40, false);
    decode(plain[..plain.len() - 1].to_vec()).unwrap();
}
