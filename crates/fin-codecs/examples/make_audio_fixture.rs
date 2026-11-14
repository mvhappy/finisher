//! Writes a stereo sine tone as audio-only MP4, for the e2e `listen` scenario (6C Task 22):
//! `cargo run -p fin-codecs --features testing --example make_audio_fixture -- <out.mp4>
//! [--codec opus|aac] [--seconds N] [--hz F] [--dbfs D]`.
//!
//! `opus` is libopus through the proxy purpose (every OS); `aac` is vo-aacenc's AAC-LC through
//! the export purpose and exists on Linux only (Decision 9). The gapless length contract of
//! Decision 22 is written (`presented` = the valid frames, `priming` from the encoder).

use std::io::Cursor;

use fin_audio::testing::signals;
use fin_codecs::{AudioEncoderConfig, open_audio_encoder};
use fin_container::{AudioTrackConfig, Mp4Writer};
use fin_media::AudioCodec;
use fin_media::backend::EncodePurpose;

const RATE: u32 = 48_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut out, mut codec) = (None, "opus".to_string());
    let (mut seconds, mut hz, mut dbfs) = (4.0f64, 440.0f64, -20.0f64);
    let num = |v: Option<String>, what: &str| -> f64 {
        v.and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{what} takes a number"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--codec" => codec = args.next().expect("--codec opus|aac"),
            "--seconds" => seconds = num(args.next(), "--seconds"),
            "--hz" => hz = num(args.next(), "--hz"),
            "--dbfs" => dbfs = num(args.next(), "--dbfs"),
            path if out.is_none() && !path.starts_with("--") => out = Some(path.to_string()),
            other => panic!("unknown argument {other}"),
        }
    }
    let out = out.expect("usage: make_audio_fixture <out.mp4> [options]");
    let (audio_codec, purpose, bitrate) = match codec.as_str() {
        "opus" => (AudioCodec::Opus, EncodePurpose::Proxy, 128_000),
        "aac" if cfg!(target_os = "linux") => (AudioCodec::Aac, EncodePurpose::Export, 192_000),
        "aac" => panic!("AAC-LC encodes on Linux only"),
        other => panic!("--codec opus|aac, not {other}"),
    };
    let frames = (seconds * f64::from(RATE)).round() as usize;
    let mut tone = signals::sine(hz, dbfs, seconds + 1.0, RATE);
    tone.truncate(frames);
    let pcm: Vec<f32> = tone.iter().flat_map(|&s| [s, s]).collect();

    let cfg = AudioEncoderConfig {
        codec: audio_codec,
        rate: RATE,
        channels: 2,
        bitrate,
    };
    let mut e = open_audio_encoder(&cfg, purpose).unwrap_or_else(|e| panic!("{}", e.message));
    let mut packets = Vec::new();
    for piece in pcm.chunks(2 * 777) {
        e.send(piece, 0).unwrap_or_else(|e| panic!("{}", e.message));
        while let Some(p) = e.receive().unwrap() {
            packets.push(p);
        }
    }
    let len = e.finish().unwrap_or_else(|e| panic!("{}", e.message));
    while let Some(p) = e.receive().unwrap() {
        packets.push(p);
    }
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Default::default()).unwrap();
    let track = w
        .add_audio(AudioTrackConfig {
            config: e.config(),
            sample_rate: RATE,
            channels: 2,
            priming: len.priming,
            presented: Some(len.valid),
        })
        .unwrap();
    for p in &packets {
        w.write(track, &p.data, p.pts, p.dts, e.frame_size(), true)
            .unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("{out}: {e}"));
    println!("{out}");
}
