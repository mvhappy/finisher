//! AAC-LC on Linux (6C plan Task 14, Decisions 9, 12 and 22): vo-aacenc → MP4 → symphonia
//! round trips, the gapless length contract through the muxer and the reader, HE-AAC refused
//! with the codec plugin as the fix, and typed failures. Lossy tolerances are stated per test.
//! Linux only: Finisher's own AAC code is in no other build (`aac_only_on_linux` is the
//! `scripts/check-crate-deps.test.mjs` case over the real workspace).
#![cfg(target_os = "linux")]

use std::io::Cursor;

use fin_audio::testing::{analyse, signals};
use fin_codecs::{
    AudioConfig, AudioEncoderConfig, AudioLimits, AudioReader, EncodedLength, EncodedPacket,
    open_audio_decoder, open_audio_encoder,
};
use fin_container::{AudioTrackConfig, CodecConfig, Demuxer, Limits, Mp4Writer};
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use fin_media::backend::{EncodePurpose, VendorClass};
use fin_media::{AudioCodec, AudioTrackInfo};

const RATE: u32 = 48_000;
const FRAME: u32 = 1024;

struct Encoded {
    packets: Vec<EncodedPacket>,
    len: EncodedLength,
    config: CodecConfig,
    frame_size: u32,
}

fn cfg(channels: u16, bitrate: u32) -> AudioEncoderConfig {
    AudioEncoderConfig {
        codec: AudioCodec::Aac,
        rate: RATE,
        channels,
        bitrate,
    }
}

fn encode(pcm: &[f32], channels: u16, bitrate: u32) -> Encoded {
    let mut e = open_audio_encoder(&cfg(channels, bitrate), EncodePurpose::Export)
        .expect("vo-aacenc encodes AAC-LC on Linux");
    let mut packets = Vec::new();
    // Uneven pieces, so the encoder's own framing is what counts.
    for piece in pcm.chunks(channels as usize * 777) {
        e.send(piece, 0).unwrap();
        while let Some(p) = e.receive().unwrap() {
            packets.push(p);
        }
    }
    let len = e.finish().unwrap();
    while let Some(p) = e.receive().unwrap() {
        packets.push(p);
    }
    assert_eq!(e.priming(), len.priming);
    Encoded {
        packets,
        len,
        config: e.config(),
        frame_size: e.frame_size(),
    }
}

fn info(channels: u16) -> AudioTrackInfo {
    AudioTrackInfo {
        codec: AudioCodec::Aac,
        fourcc: "mp4a".into(),
        sample_rate: RATE,
        channels,
        priming: 0,
        padding: 0,
    }
}

/// Every decoded sample of `enc` (priming and padding included) and the layout.
fn decode_all(enc: &Encoded, channels: u16) -> (Vec<f32>, ChannelLayout) {
    let mut d =
        open_audio_decoder(&info(channels), &AudioConfig::Record(enc.config.clone())).unwrap();
    assert_eq!(d.backend().0, "symphonia");
    let (mut pcm, mut layout) = (Vec::new(), ChannelLayout::default());
    for p in &enc.packets {
        d.send(&p.data, p.pts).unwrap();
        while let Some(a) = d.receive().unwrap() {
            assert_eq!(a.rate, RATE);
            layout = a.layout;
            pcm.extend(a.interleaved);
        }
    }
    d.finish().unwrap();
    while let Some(a) = d.receive().unwrap() {
        pcm.extend(a.interleaved);
    }
    (pcm, layout)
}

/// Decoded samples aligned with the input: from `priming`, `valid` frames.
fn aligned(enc: &Encoded, channels: u16) -> Vec<f32> {
    let (pcm, _) = decode_all(enc, channels);
    let n = channels as usize;
    let from = enc.len.priming as usize * n;
    pcm[from..from + enc.len.valid as usize * n].to_vec()
}

fn channel(pcm: &[f32], n: usize, c: usize) -> Vec<f32> {
    pcm.iter().skip(c).step_by(n).copied().collect()
}

fn interleave(chans: &[Vec<f32>]) -> Vec<f32> {
    (0..chans[0].len())
        .flat_map(|i| chans.iter().map(move |c| c[i]))
        .collect()
}

/// An MP4 of `enc` with `presented = valid` (Decision 22).
fn mp4(enc: &Encoded, channels: u16) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let a = w
        .add_audio(AudioTrackConfig {
            config: enc.config.clone(),
            sample_rate: RATE,
            channels,
            priming: enc.len.priming,
            presented: Some(enc.len.valid),
        })
        .unwrap();
    for p in &enc.packets {
        w.write(a, &p.data, p.pts, p.dts, enc.frame_size, true)
            .unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn read(bytes: Vec<u8>) -> Result<(AudioReader, Vec<f32>, ChannelLayout), fin_core::CoreError> {
    let mut r = AudioReader::open(Cursor::new(bytes), &AudioLimits::default())?;
    let (mut pcm, mut layout) = (Vec::new(), ChannelLayout::default());
    while let Some(c) = r.next()? {
        layout = c.layout;
        pcm.extend(c.interleaved);
    }
    Ok((r, pcm, layout))
}

fn stereo_sine(hz: f64, dbfs: f64, frames: usize) -> Vec<f32> {
    let secs = frames as f64 / RATE as f64;
    let mut s = signals::sine(hz, dbfs, secs + 1.0, RATE);
    s.truncate(frames);
    interleave(&[s.clone(), s])
}

/// `Σ a[i]·b[i + lag]` over the overlap.
fn corr(a: &[f32], b: &[f32], lag: i64) -> f64 {
    (0..a.len() as i64)
        .filter_map(|i| {
            let j = i + lag;
            (j >= 0 && (j as usize) < b.len()).then(|| a[i as usize] as f64 * b[j as usize] as f64)
        })
        .sum()
}

#[test]
fn aac_lc_round_trip() {
    // 1 kHz at −6 dBFS, 192 kb/s stereo (6E's template rate): level ±0.3 dB, the residual after
    // aligning by `priming` (1,024) at most −20 dB.
    let src = stereo_sine(1000.0, -6.0, RATE as usize);
    let enc = encode(&src, 2, 192_000);
    assert_eq!(enc.len.priming, 1024);
    assert_eq!(enc.len.valid, RATE as u64);
    let CodecConfig::Aac { .. } = &enc.config else {
        panic!("an esds record")
    };
    let out = aligned(&enc, 2);
    for c in 0..2 {
        let (a, b) = (channel(&src, 2, c), channel(&out, 2, c));
        let level = analyse::rms_dbfs(&b) - analyse::rms_dbfs(&a);
        assert!(level.abs() <= 0.3, "channel {c}: level {level:.3} dB");
        // (No zero-crossing frequency check: the codec's noise floor adds crossings.)
        let null = analyse::null_db(&a, &b);
        assert!(null <= -20.0, "channel {c}: residual {null:.1} dB");
    }
    // Aligned by the priming, the decoded signal lines up with the input (no lag).
    let (a, b) = (channel(&src, 2, 0), channel(&out, 2, 0));
    let lag = (-8i64..=8)
        .max_by(|&x, &y| corr(&a, &b, x).total_cmp(&corr(&a, &b, y)))
        .unwrap();
    assert_eq!(lag, 0, "the priming is the encoder's whole delay");
    // Pink noise: RMS within ±0.2 dB.
    let src = interleave(&[signals::pink(7, 2.0, RATE), signals::pink(8, 2.0, RATE)]);
    let enc = encode(&src, 2, 192_000);
    let out = aligned(&enc, 2);
    for c in 0..2 {
        let (a, b) = (channel(&src, 2, c), channel(&out, 2, c));
        let level = analyse::rms_dbfs(&b) - analyse::rms_dbfs(&a);
        assert!(level.abs() <= 0.2, "pink channel {c}: level {level:.3} dB");
    }
    // Through the MP4 muxer and the reader: the same samples, the right backend and class.
    let enc = encode(&stereo_sine(1000.0, -6.0, 30_001), 2, 192_000);
    let (r, pcm, layout) = read(mp4(&enc, 2)).unwrap();
    assert_eq!(
        (r.info().codec, r.backend().0),
        (AudioCodec::Aac, "symphonia")
    );
    assert_eq!(r.decoder_class(), VendorClass::Software);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::L, ChannelPos::R]));
    assert_eq!(pcm.len(), 30_001 * 2);
}

#[test]
fn aac_mono_round_trip() {
    let mut mono = signals::sine(1000.0, -6.0, 1.0, RATE);
    mono.truncate(RATE as usize);
    let enc = encode(&mono, 1, 96_000);
    let (_, layout) = decode_all(&enc, 1);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::C]));
    let out = aligned(&enc, 1);
    let level = analyse::rms_dbfs(&out) - analyse::rms_dbfs(&mono);
    assert!(level.abs() <= 0.3, "mono level {level:.3} dB");
}

#[test]
fn aac_gapless_lengths() {
    for n in [1usize, 1_023, 1_024, 1_025, 48_000, 48_001] {
        let src = stereo_sine(440.0, -6.0, n);
        let enc = encode(&src, 2, 192_000);
        let EncodedLength {
            priming,
            valid,
            padding,
        } = enc.len;
        assert_eq!(valid, n as u64, "valid is the samples sent");
        assert_eq!(priming, 1024, "{n}");
        assert!(padding < FRAME, "{n}: {:?}", enc.len);
        let (all, _) = decode_all(&enc, 2);
        assert_eq!(
            (all.len() / 2) as u64,
            priming as u64 + valid + padding as u64,
            "{n}: the decoded total"
        );
        let bytes = mp4(&enc, 2);
        let d = Demuxer::open(Cursor::new(bytes.clone()), &Limits::default()).unwrap();
        let a = d.info().audio.clone().unwrap();
        assert_eq!(
            (a.priming, a.padding),
            (1024, padding),
            "{n}: the edit's media time and the demuxer's padding"
        );
        let (_, pcm, _) = read(bytes).unwrap();
        assert_eq!(pcm.len(), n * 2, "{n}: exactly the samples sent");
        let from = priming as usize * 2;
        assert!(
            pcm.iter()
                .zip(&all[from..])
                .all(|(x, y)| x.to_bits() == y.to_bits()),
            "{n}: the reader trims exactly the priming"
        );
    }
}

/// An `esds` body around `asc` (as `fin-container`'s tests write one).
fn esds(asc: &[u8]) -> Vec<u8> {
    let dsi = [&[0x05, asc.len() as u8][..], asc].concat();
    let mut dcd = vec![0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    dcd.extend(dsi);
    let dcd = [&[0x04, dcd.len() as u8][..], &dcd].concat();
    let es = [&[0u8, 1, 0][..], &dcd, &[0x06, 1, 2]].concat();
    [&[0u8, 0, 0, 0, 0x03, es.len() as u8][..], &es].concat()
}

fn open_esds(asc: &[u8]) -> Result<Box<dyn fin_codecs::AudioDecoder>, fin_core::CoreError> {
    open_audio_decoder(
        &info(2),
        &AudioConfig::Record(CodecConfig::Aac { esds: esds(asc) }),
    )
}

#[test]
fn he_aac_refused_with_plugin_fix() {
    // Explicit SBR: object type 5, core 24 kHz, stereo, extension 48 kHz, then AAC-LC.
    // Then backward-compatible SBR after an LC GASpecificConfig, without and with
    // extensionFlag (whose extensionFlag3 bit precedes the sync extension).
    for asc in [
        &[0x2B, 0x11, 0x88, 0x00][..],
        &[0x13, 0x10, 0x56, 0xE5, 0x98],
        &[0x13, 0x11, 0x2b, 0x72, 0xcc],
    ] {
        let e = open_esds(asc).err().expect("HE-AAC is refused");
        assert_eq!(e.code, "CODEC_UNAVAILABLE", "{asc:?}");
        assert_eq!(e.details["codec"], "aac");
        assert_eq!(e.details["profile"], "he-aac");
        assert_eq!(e.details["os"], "linux");
        assert_eq!(e.details["fix"], "codecPlugin");
        assert!(e.fix.is_some());
    }
    // Plain LC at the same rate opens.
    assert!(open_esds(&[0x11, 0x90]).is_ok());
}

#[test]
fn aac_corrupt_typed() {
    // A cut or foreign configuration record.
    for asc in [&[0x11][..], &[]] {
        let e = open_esds(asc).err().expect("refused");
        assert_eq!(e.code, "MEDIA_CORRUPT", "{asc:?}");
    }
    // Main profile (1) and 5.1 (channel configuration 6) are not this decoder's.
    let e = open_esds(&[0x09, 0x90]).err().expect("main profile");
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("CODEC_UNAVAILABLE", &"profile".into())
    );
    let e = open_esds(&[0x11, 0xB0]).err().expect("5.1");
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("CODEC_UNAVAILABLE", &"channels".into())
    );
    // Garbage and cut packets: typed, never a panic (a garbage packet may also parse, e.g. one
    // whose first element is ID_END).
    let enc = encode(&stereo_sine(1000.0, -6.0, 4800), 2, 192_000);
    let mut seed = 0x2545_f491_u32;
    let mut garbage: Vec<Vec<u8>> = (0..64)
        .map(|i| {
            (0..1 + i * 7)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect()
        })
        .collect();
    let cut: Vec<Vec<u8>> = enc.packets[1..5]
        .iter()
        .map(|p| p.data[..p.data.len() / 2].to_vec())
        .collect();
    garbage.extend(cut.iter().cloned());
    let typed = |bad: &[u8], e: &fin_core::CoreError| {
        assert!(
            matches!(e.code.as_str(), "DECODE_FAILED" | "CODEC_UNAVAILABLE"),
            "{bad:?}: {e:?}"
        );
        assert_eq!(e.details["backend"], "symphonia");
    };
    for bad in &garbage {
        let mut d = open_audio_decoder(&info(2), &AudioConfig::Record(enc.config.clone())).unwrap();
        if let Err(e) = d.send(bad, 0) {
            typed(bad, &e);
        }
    }
    // Every truncated packet fails, each with a typed error.
    for (i, bad) in cut.iter().enumerate() {
        let mut d = open_audio_decoder(&info(2), &AudioConfig::Record(enc.config.clone())).unwrap();
        let e = d
            .send(bad, 0)
            .err()
            .unwrap_or_else(|| panic!("cut packet {i} decoded"));
        typed(bad, &e);
    }
    // A cut packet inside a file: typed through the reader.
    let mut broken = encode(&stereo_sine(1000.0, -6.0, 4800), 2, 192_000);
    broken.packets[2].data = cut[1].clone();
    let e = read(mp4(&broken, 2)).unwrap_err();
    assert!(
        matches!(e.code.as_str(), "DECODE_FAILED" | "CODEC_UNAVAILABLE"),
        "{e:?}"
    );
    // A cut file (Task 12's contract): a typed error, never a panic and never a silent short
    // read, at every cut point.
    let bytes = mp4(&enc, 2);
    for cut in (1..10)
        .map(|k| bytes.len() * k / 10)
        .chain([bytes.len() - 1])
    {
        let e = read(bytes[..cut].to_vec())
            .err()
            .unwrap_or_else(|| panic!("{cut} of {} bytes read without an error", bytes.len()));
        assert!(
            matches!(
                e.code.as_str(),
                "MEDIA_CORRUPT" | "DECODE_FAILED" | "UNSUPPORTED_CONTAINER"
            ),
            "{cut}: {e:?}"
        );
    }
}

#[test]
fn aac_encoder_refuses_what_it_cannot_encode() {
    for c in [
        AudioEncoderConfig {
            rate: 44_000,
            ..cfg(2, 192_000)
        },
        cfg(0, 192_000),
        cfg(6, 384_000),
        cfg(2, 1_000),
        cfg(2, 400_000),
    ] {
        let e = open_audio_encoder(&c, EncodePurpose::Export)
            .err()
            .expect("refused");
        assert_eq!(e.code, "CODEC_UNAVAILABLE", "{c:?}");
    }
    let mut e = open_audio_encoder(&cfg(2, 192_000), EncodePurpose::Export).unwrap();
    assert_eq!(
        e.send(&[0.0, f32::NAN], 0).unwrap_err().code,
        "ENCODE_FAILED"
    );
    assert_eq!(e.send(&[0.0], 0).unwrap_err().code, "ENCODE_FAILED");
    // Software encoders serve every purpose (Decision 10).
    for p in [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Proxy,
    ] {
        assert!(open_audio_encoder(&cfg(2, 192_000), p).is_ok(), "{p:?}");
    }
}
