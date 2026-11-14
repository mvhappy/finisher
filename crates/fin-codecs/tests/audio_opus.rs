//! libopus decode and encode (6C plan Task 13, Decisions 3, 11 and 22): round trips through our
//! own encoder, the gapless length contract through the MP4 muxer, Ogg Opus through the test
//! writer, mapping families 0 and 1, and typed failures. Lossy tolerances are stated per test.

use std::io::Cursor;

use fin_audio::channels::{Canonical, canonical};
use fin_audio::testing::{analyse, signals};
use fin_codecs::audio::testing::ogg;
use fin_codecs::{
    AudioConfig, AudioEncoderConfig, AudioLimits, AudioReader, EncodedLength, EncodedPacket,
    open_audio_decoder, open_audio_encoder,
};
use fin_container::{AudioTrackConfig, CodecConfig, Demuxer, Limits, Mp4Writer};
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use fin_media::backend::{EncodePurpose, VendorClass};
use fin_media::{AudioCodec, AudioTrackInfo};

const RATE: u32 = 48_000;

struct Encoded {
    packets: Vec<EncodedPacket>,
    len: EncodedLength,
    config: CodecConfig,
    frame_size: u32,
}

fn encode(pcm: &[f32], channels: u16, bitrate: u32) -> Encoded {
    let cfg = AudioEncoderConfig {
        codec: AudioCodec::Opus,
        rate: RATE,
        channels,
        bitrate,
    };
    let mut e = open_audio_encoder(&cfg, EncodePurpose::Proxy).expect("libopus encodes Opus");
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

fn info(channels: u16, priming: u32) -> AudioTrackInfo {
    AudioTrackInfo {
        codec: AudioCodec::Opus,
        fourcc: "Opus".into(),
        sample_rate: RATE,
        channels,
        priming,
        padding: 0,
    }
}

/// Every decoded sample of `enc` (priming and padding included) and the layout.
fn decode_all(enc: &Encoded, channels: u16) -> (Vec<f32>, ChannelLayout) {
    let mut d = open_audio_decoder(
        &info(channels, enc.len.priming),
        &AudioConfig::Record(enc.config.clone()),
    )
    .unwrap();
    assert_eq!(d.backend().0, "libopus");
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

/// An MP4 of `enc` with `presented = valid` (Decision 22), beside a 30,000-timescale video
/// track when `video`.
fn mp4(enc: &Encoded, channels: u16, video: bool) -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let v = video.then(|| {
        w.add_video(fin_container::VideoTrackConfig {
            config: CodecConfig::Other { fourcc: *b"test" },
            coded: fin_media::Size::new(16, 16),
            timescale: 30_000,
            rotation: fin_media::Rotation::R0,
            pixel_aspect: fin_core::Rational::from_int(1),
            colour: fin_media::ColourTags::BT709,
            hdr: None,
        })
        .unwrap()
    });
    let a = w
        .add_audio(AudioTrackConfig {
            config: enc.config.clone(),
            sample_rate: RATE,
            channels,
            priming: enc.len.priming,
            presented: Some(enc.len.valid),
        })
        .unwrap();
    if let Some(v) = v {
        for i in 0..30i64 {
            w.write(v, &[1, 2, 3], i * 1001, i * 1001, 1001, true)
                .unwrap();
        }
    }
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

#[test]
fn opus_round_trip_sine() {
    let src = stereo_sine(1000.0, -6.0, RATE as usize);
    let enc = encode(&src, 2, 128_000);
    assert_eq!(enc.len.valid, RATE as u64);
    let out = aligned(&enc, 2);
    for c in 0..2 {
        let (a, b) = (channel(&src, 2, c), channel(&out, 2, c));
        let level = analyse::rms_dbfs(&b) - analyse::rms_dbfs(&a);
        assert!(level.abs() <= 0.3, "channel {c}: level {level:.3} dB");
        let hz = analyse::dominant_hz(&b, RATE);
        assert!((hz - 1000.0).abs() <= 1.0, "channel {c}: {hz:.2} Hz");
        let null = analyse::null_db(&a, &b);
        assert!(null <= -25.0, "channel {c}: residual {null:.1} dB");
    }
}

/// A first-order high-pass at `hz` (`f64` state).
fn high_pass(x: &[f32], hz: f64) -> Vec<f32> {
    let a = 1.0 / (1.0 + std::f64::consts::TAU * hz / RATE as f64);
    let (mut px, mut py) = (0.0f64, 0.0f64);
    x.iter()
        .map(|&v| {
            let y = a * (py + v as f64 - px);
            (px, py) = (v as f64, y);
            y as f32
        })
        .collect()
}

#[test]
fn opus_round_trip_level() {
    // Pink noise in the audible band: the Voss-McCartney rows that refresh every few thousand
    // samples put real energy below 20 Hz, which libopus's encoder removes on purpose (its DC
    // reject), so the unfiltered signal reads 0.5 dB low at every bitrate.
    let l = high_pass(&signals::pink(7, 2.0, RATE), 20.0);
    let r = high_pass(&signals::pink(8, 2.0, RATE), 20.0);
    let src = interleave(&[l, r]);
    let enc = encode(&src, 2, 128_000);
    let out = aligned(&enc, 2);
    for c in 0..2 {
        let (a, b) = (channel(&src, 2, c), channel(&out, 2, c));
        let level = analyse::rms_dbfs(&b) - analyse::rms_dbfs(&a);
        assert!(level.abs() <= 0.3, "channel {c}: level {level:.3} dB");
    }
}

#[test]
fn opus_gapless_lengths() {
    for n in [0usize, 1, 959, 960, 961, 48_000, 48_001] {
        let src = stereo_sine(440.0, -6.0, n);
        let enc = encode(&src, 2, 128_000);
        let EncodedLength {
            priming,
            valid,
            padding,
        } = enc.len;
        assert_eq!(valid, n as u64, "valid is the samples sent");
        assert!(
            priming > 0 && padding < enc.frame_size,
            "{n}: {:?}",
            enc.len
        );
        let (all, _) = decode_all(&enc, 2);
        assert_eq!(
            (all.len() / 2) as u64,
            priming as u64 + valid + padding as u64,
            "{n}: the decoded total"
        );
        let d = Demuxer::open(Cursor::new(mp4(&enc, 2, false)), &Limits::default()).unwrap();
        let a = d.info().audio.clone().unwrap();
        assert_eq!((a.priming, a.padding), (priming, padding), "{n}");
        let (r, pcm, _) = read(mp4(&enc, 2, false)).unwrap();
        assert_eq!(r.info().codec, AudioCodec::Opus);
        assert_eq!(pcm.len(), n * 2, "{n}: exactly the samples sent");
        // The reader's first sample is the decoder's first after the priming (the input's first).
        let from = priming as usize * 2;
        assert!(
            pcm.iter()
                .zip(&all[from..])
                .all(|(x, y)| x.to_bits() == y.to_bits()),
            "{n}: the reader trims exactly the priming"
        );
    }
    // Aligned by the priming, the decoded signal lines up with the input (no lag).
    let src = stereo_sine(440.0, -6.0, 48_000);
    let out = aligned(&encode(&src, 2, 128_000), 2);
    let (a, b) = (channel(&src, 2, 0), channel(&out, 2, 0));
    let lag = (-8i64..=8)
        .max_by(|&x, &y| corr(&a, &b, x).total_cmp(&corr(&a, &b, y)))
        .unwrap();
    assert_eq!(lag, 0);
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
fn mux_edit_exact_with_video() {
    let enc = encode(&stereo_sine(440.0, -6.0, 48_001), 2, 128_000);
    let bytes = mp4(&enc, 2, true);
    let d = Demuxer::open(Cursor::new(bytes.clone()), &Limits::default()).unwrap();
    let a = d.info().audio.clone().unwrap();
    assert_eq!(
        (a.priming, a.padding),
        (enc.len.priming, enc.len.padding),
        "the demuxer reads back the encoder's padding"
    );
    let (_, pcm, _) = read(bytes).unwrap();
    assert_eq!(pcm.len(), 48_001 * 2);
}

#[test]
fn opus_in_ogg_test_writer() {
    let src = stereo_sine(1000.0, -6.0, 30_001);
    let enc = encode(&src, 2, 128_000);
    let CodecConfig::Opus { dops } = &enc.config else {
        panic!("dOps")
    };
    let packets: Vec<Vec<u8>> = enc.packets.iter().map(|p| p.data.clone()).collect();
    let file = ogg::opus(dops, &packets, enc.frame_size, enc.len.valid);
    let (r, pcm, layout) = read(file).unwrap();
    assert_eq!(
        (r.info().codec, r.info().priming, r.backend().0),
        (AudioCodec::Opus, enc.len.priming, "libopus")
    );
    assert_eq!(r.decoder_class(), VendorClass::Software);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::L, ChannelPos::R]));
    assert_eq!(pcm.len(), 30_001 * 2, "the last granule cuts the padding");
    let (_, from_mp4, _) = read(mp4(&enc, 2, false)).unwrap();
    assert!(
        pcm.iter()
            .zip(&from_mp4)
            .all(|(x, y)| x.to_bits() == y.to_bits()),
        "Ogg and MP4 give the same samples"
    );
}

#[test]
fn opus_mono_and_51() {
    // Mono: family 0, one channel, reported as the centre.
    let mut mono = signals::sine(1000.0, -6.0, 1.0, RATE);
    mono.truncate(RATE as usize);
    let enc = encode(&mono, 1, 64_000);
    let (_, layout) = decode_all(&enc, 1);
    assert_eq!(layout, ChannelLayout(vec![ChannelPos::C]));
    let out = aligned(&enc, 1);
    let level = analyse::rms_dbfs(&out) - analyse::rms_dbfs(&mono);
    assert!(level.abs() <= 0.3, "mono level {level:.3} dB");
    // 5.1: family 1 in Vorbis order (L C R Ls Rs LFE); 1 kHz in C, 440 Hz in Ls, 60 Hz in LFE.
    let n = RATE as usize;
    let tone = |hz: f64| {
        let mut s = signals::sine(hz, -6.0, 1.0, RATE);
        s.truncate(n);
        s
    };
    let z = vec![0.0f32; n];
    let src = interleave(&[
        z.clone(),
        tone(1000.0),
        z.clone(),
        tone(440.0),
        z.clone(),
        tone(60.0),
    ]);
    let enc = encode(&src, 6, 320_000);
    let CodecConfig::Opus { dops } = &enc.config else {
        panic!("dOps")
    };
    assert_eq!((dops[1], dops[10]), (6, 1), "six channels, family 1");
    let (_, layout) = decode_all(&enc, 6);
    assert_eq!(layout, ChannelLayout::vorbis_order(6));
    let out = aligned(&enc, 6);
    let Canonical::Stereo(s) = canonical(&out, &layout).unwrap() else {
        panic!("5.1 downmixes to stereo")
    };
    // Decision 3: C at −3 dB to both sides, Ls at −3 dB to the left only, LFE dropped.
    let at = |x: &[f32], hz: f64| analyse::goertzel_db(x, hz, RATE);
    for (side, x) in [("L", &s.l), ("R", &s.r)] {
        let c = at(x, 1000.0);
        assert!((c - (-9.0)).abs() <= 0.5, "{side}: centre {c:.2} dB");
        assert!(at(x, 60.0) <= -40.0, "{side}: the LFE is dropped");
    }
    let ls = at(&s.l, 440.0);
    assert!((ls - (-9.0)).abs() <= 0.5, "L: surround {ls:.2} dB");
    assert!(at(&s.r, 440.0) <= -30.0, "R: no left surround");
}

#[test]
fn opus_corrupt_packet_typed() {
    let enc = encode(&stereo_sine(1000.0, -6.0, 4800), 2, 128_000);
    let config = AudioConfig::Record(enc.config.clone());
    // Code 3 with no frames, code 1 with an odd body, an empty packet, 61 frames of 20 ms.
    for bad in [&[0x03u8, 0x00][..], &[0x01, 0x00], &[], &[0x0B, 0x3D, 0, 0]] {
        let mut d = open_audio_decoder(&info(2, 312), &config).unwrap();
        let e = d.send(bad, 0).unwrap_err();
        assert_eq!(e.code, "DECODE_FAILED", "{bad:?}");
        assert_eq!(e.details["backend"], "libopus");
    }
    // A corrupt packet inside a file: typed through the reader, never a panic.
    let mut broken = enc;
    broken.packets[2].data = vec![0x03, 0x00];
    let e = read(mp4(&broken, 2, false)).unwrap_err();
    assert_eq!(e.code, "DECODE_FAILED");
    // Family 255 (and any family past 1) is not decoded here; more than 8 channels neither.
    let mut dops = vec![0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 255, 1, 1, 0, 1];
    let unavailable = |dops: &[u8], ch: u16| {
        open_audio_decoder(
            &info(ch, 312),
            &AudioConfig::Record(CodecConfig::Opus {
                dops: dops.to_vec(),
            }),
        )
        .err()
        .expect("refused")
    };
    let e = unavailable(&dops, 2);
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("CODEC_UNAVAILABLE", &"channelMapping".into())
    );
    dops[10] = 2;
    assert_eq!(unavailable(&dops, 2).code, "CODEC_UNAVAILABLE");
    // A cut configuration record is corrupt.
    let e = unavailable(&dops[..6], 2);
    assert_eq!(e.code, "MEDIA_CORRUPT");
    // A family 1 table naming a channel past the decoded ones is corrupt.
    let bad = [0, 2, 0x01, 0x38, 0, 0, 0xBB, 0x80, 0, 0, 1, 1, 1, 0, 9];
    assert_eq!(unavailable(&bad, 2).code, "MEDIA_CORRUPT");
}

#[test]
fn opus_encoder_refuses_what_it_cannot_encode() {
    let cfg = |rate: u32, channels: u16| AudioEncoderConfig {
        codec: AudioCodec::Opus,
        rate,
        channels,
        bitrate: 128_000,
    };
    for (rate, ch) in [(44_100, 2), (48_000, 0), (48_000, 9)] {
        let e = open_audio_encoder(&cfg(rate, ch), EncodePurpose::Proxy)
            .err()
            .expect("refused");
        assert_eq!(e.code, "CODEC_UNAVAILABLE", "{rate} {ch}");
    }
    // Software encoders serve every purpose (Decision 10).
    for p in [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Proxy,
    ] {
        assert!(open_audio_encoder(&cfg(48_000, 2), p).is_ok(), "{p:?}");
    }
}

// Ogg Opus granule positions (RFC 7845 §4): the reader trims from the stream's own start offset,
// pre-skip and last granule, with packet durations from the TOC (RFC 6716 §3.1), and counts
// every decoded frame against the budget.

/// Mono, pre-skip `pre_skip`, 48 kHz, gain 0, family 0.
fn mono_dops(pre_skip: u16) -> Vec<u8> {
    let p = pre_skip.to_be_bytes();
    vec![0, 1, p[0], p[1], 0, 0, 0xBB, 0x80, 0, 0, 0]
}

/// An Ogg Opus file of `audio` `(packet, granule)` pages behind the two headers.
fn ogg_opus(dops: &[u8], audio: &[(Vec<u8>, i64)]) -> Vec<u8> {
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&[0; 8]);
    let mut packets = vec![(ogg::opus_head(dops), 0), (tags, 0)];
    packets.extend_from_slice(audio);
    ogg::ogg(42, &packets)
}

/// One 20 ms CELT packet (config 31, code 0): 960 frames.
fn celt_20ms() -> Vec<u8> {
    vec![0xF8, 0xFF, 0xFE]
}

fn read_with(
    bytes: Vec<u8>,
    limits: &AudioLimits,
) -> Result<(Option<u64>, usize), fin_core::CoreError> {
    let mut r = AudioReader::open(Cursor::new(bytes), limits)?;
    let declared = r.declared_frames();
    let mut frames = 0;
    while let Some(c) = r.next()? {
        frames += c.interleaved.len() / c.layout.channels();
    }
    Ok((declared, frames))
}

#[test]
fn ogg_opus_decoded_frames_count_against_the_budget() {
    let limits = AudioLimits {
        max_seconds: 1,
        ..AudioLimits::default()
    };
    // 1,000 packets of 960 frames (960,000 frames) whose pages all say granule 1: the first
    // audio page holds more samples than its granule states, which RFC 7845 §4.5 rejects.
    let audio: Vec<_> = (0..1000).map(|_| (celt_20ms(), 1)).collect();
    let e = read_with(ogg_opus(&mono_dops(0), &audio), &limits).unwrap_err();
    assert!(
        matches!(e.code.as_str(), "MEDIA_CORRUPT" | "MEDIA_LIMIT"),
        "{e:?}"
    );
    // A valid first page (granule 960), then 999 pages that say granule 1: the stream presents
    // one frame, and the reader stops there instead of decoding 959,040 frames it drops.
    let mut audio = vec![(celt_20ms(), 960)];
    audio.extend((0..999).map(|_| (celt_20ms(), 1)));
    let got = read_with(ogg_opus(&mono_dops(0), &audio), &limits).unwrap();
    assert_eq!(got, (Some(1), 1));
}

#[test]
fn ogg_opus_120ms_packets_trim_to_the_last_granule() {
    // Config 28 (CELT 2.5 ms), code 3, M = 48 CBR frames of 2 bytes: 5,760 frames (120 ms).
    let mut packet = vec![0xE3, 48];
    for _ in 0..48 {
        packet.extend_from_slice(&[0xFF, 0xFE]);
    }
    let dops = mono_dops(0);
    let config = AudioConfig::Record(CodecConfig::Opus { dops: dops.clone() });
    let mut d = open_audio_decoder(&info(1, 0), &config).unwrap();
    d.send(&packet, 0).unwrap();
    assert_eq!(d.receive().unwrap().unwrap().interleaved.len(), 5760);
    // Two of them (11,520 frames) whose last granule is 11,000: 520 frames of padding.
    let file = ogg::opus(&dops, &[packet.clone(), packet], 5760, 11_000);
    let got = read_with(file, &AudioLimits::default()).unwrap();
    assert_eq!(got, (Some(11_000), 11_000));
}

#[test]
fn ogg_opus_nonzero_start_granule() {
    // The first audio page ends at granule 48,960 with 960 samples: the stream starts at
    // 48,000, so it holds 1,920 frames (RFC 7845 §4).
    let audio = [(celt_20ms(), 48_960), (celt_20ms(), 49_920)];
    let got = read_with(ogg_opus(&mono_dops(0), &audio), &AudioLimits::default()).unwrap();
    assert_eq!(got, (Some(1920), 1920));
    // With a pre-skip of 100 the start offset still comes off first, then the pre-skip.
    let got = read_with(ogg_opus(&mono_dops(100), &audio), &AudioLimits::default()).unwrap();
    assert_eq!(got, (Some(1820), 1820));
    // The same packets from granule 0 present the full two packets minus the pre-skip.
    let audio = [(celt_20ms(), 960), (celt_20ms(), 1920)];
    let got = read_with(ogg_opus(&mono_dops(100), &audio), &AudioLimits::default()).unwrap();
    assert_eq!(got, (Some(1820), 1820));
}

/// Rewrites an Ogg page's CRC after an edit.
fn recrc(page: &mut [u8]) {
    page[22..26].fill(0);
    let mut c = 0u32;
    for &b in page.iter() {
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            };
        }
    }
    page[22..26].copy_from_slice(&c.to_le_bytes());
}

#[test]
fn ogg_opus_orphaned_continuation_is_skipped() {
    // The headers, then a continuation page (sequence 2) whose first segment is the tail of a
    // packet that is not in the file (`[0xE3, 0]`, which would be a forbidden code 3 packet),
    // then a whole 960-frame packet ending at granule 48,960; the last page ends at 49,920.
    let dops = mono_dops(0);
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&[0; 8]);
    let mut file = ogg::ogg(42, &[(ogg::opus_head(&dops), 0), (tags, 0)]);
    let tags_page = file.len() - (27 + 1 + 16);
    file[tags_page + 5] = 0;
    recrc(&mut file[tags_page..]);
    let mut page = ogg::ogg(42, &[(vec![0xE3, 0, 0xF8, 0xFF, 0xFE], 48_960)]);
    page[5] = 1;
    page[18..22].copy_from_slice(&2u32.to_le_bytes());
    page[26] = 2;
    page[27] = 2;
    page.insert(28, 3);
    recrc(&mut page);
    file.extend(page);
    let mut end = ogg::ogg(42, &[(celt_20ms(), 49_920)]);
    end[5] = 4;
    end[18..22].copy_from_slice(&3u32.to_le_bytes());
    recrc(&mut end);
    file.extend(end);
    let got = read_with(file, &AudioLimits::default()).unwrap();
    assert_eq!(got, (Some(1920), 1920));
}

#[test]
fn ogg_opus_end_before_the_pre_skip_is_corrupt() {
    // Pre-skip 100, one 960-frame packet, and the last granule 50: RFC 7845 §4.5 rejects it.
    let e = read_with(
        ogg_opus(&mono_dops(100), &[(celt_20ms(), 50)]),
        &AudioLimits::default(),
    )
    .unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
}

#[test]
fn ogg_opus_pre_skip_counts_against_the_budget() {
    // A valid stream: pre-skip 65,535 and 70 packets of 960 frames (67,200 decoded, 1,665
    // presented) under a one-second budget (48,000 frames): the pre-skip is decoded, so it
    // counts.
    let audio: Vec<_> = (0..70).map(|i| (celt_20ms(), (i + 1) * 960)).collect();
    let limits = AudioLimits {
        max_seconds: 1,
        ..AudioLimits::default()
    };
    let e = read_with(ogg_opus(&mono_dops(65_535), &audio), &limits).unwrap_err();
    assert_eq!(e.code, "MEDIA_LIMIT", "{e:?}");
}
