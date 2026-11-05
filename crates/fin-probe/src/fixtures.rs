//! Test-time MP4 fixture writer (feature `fixtures`). No media is committed to the repo.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use bytes::Bytes;
use mp4::{
    AacConfig, AudioObjectType, AvcConfig, ChannelConfig, MediaConfig, Mp4Config, Mp4Sample,
    Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
};

/// Shape of the generated file: an H.264-tagged video track (320x180) and, optionally, a
/// stereo AAC-tagged audio track. Sample payloads are dummy bytes.
#[derive(Debug, Clone, Copy)]
pub struct FixtureOpts {
    pub seconds: u32,
    pub fps: u32,
    pub audio: bool,
    /// One of 96000, 48000, 44100, 32000, 24000, 22050, 16000, 8000.
    pub sample_rate: u32,
}

impl Default for FixtureOpts {
    /// 2 s, 25 fps, 48 kHz audio.
    fn default() -> Self {
        FixtureOpts {
            seconds: 2,
            fps: 25,
            audio: true,
            sample_rate: 48_000,
        }
    }
}

fn freq_index(rate: u32) -> SampleFreqIndex {
    match rate {
        96_000 => SampleFreqIndex::Freq96000,
        48_000 => SampleFreqIndex::Freq48000,
        44_100 => SampleFreqIndex::Freq44100,
        32_000 => SampleFreqIndex::Freq32000,
        24_000 => SampleFreqIndex::Freq24000,
        22_050 => SampleFreqIndex::Freq22050,
        16_000 => SampleFreqIndex::Freq16000,
        8_000 => SampleFreqIndex::Freq8000,
        other => panic!("unsupported fixture sample rate {other}"),
    }
}

pub fn write_fixture_mp4(path: &Path, opts: FixtureOpts) {
    write(path, opts, "isom", &["isom", "mp41"]);
}

/// [`write_fixture_mp4`] as a QuickTime file: `ftyp` major brand `qt  `.
pub fn write_fixture_mov(path: &Path, opts: FixtureOpts) {
    write(path, opts, "qt  ", &["qt  "]);
}

fn write(path: &Path, opts: FixtureOpts, brand: &str, compatible: &[&str]) {
    let file = BufWriter::new(File::create(path).unwrap());
    let config = Mp4Config {
        major_brand: brand.parse().unwrap(),
        minor_version: 512,
        compatible_brands: compatible.iter().map(|b| b.parse().unwrap()).collect(),
        timescale: 1000,
    };
    let mut w = Mp4Writer::write_start(file, &config).unwrap();
    let track = |track_type, timescale, media_conf| TrackConfig {
        track_type,
        timescale,
        language: "und".into(),
        media_conf,
    };
    let avc = AvcConfig {
        width: 320,
        height: 180,
        seq_param_set: vec![0x67, 0x42, 0x00, 0x1e, 0x95, 0xa8],
        pic_param_set: vec![0x68, 0xce, 0x3c, 0x80],
    };
    // Timescale fps*1000 with 1000-tick samples gives an exact fps rate.
    w.add_track(&track(
        TrackType::Video,
        opts.fps * 1000,
        MediaConfig::AvcConfig(avc),
    ))
    .unwrap();
    if opts.audio {
        let aac = AacConfig {
            bitrate: 128_000,
            profile: AudioObjectType::AacLowComplexity,
            freq_index: freq_index(opts.sample_rate),
            chan_conf: ChannelConfig::Stereo,
        };
        w.add_track(&track(
            TrackType::Audio,
            opts.sample_rate,
            MediaConfig::AacConfig(aac),
        ))
        .unwrap();
    }
    for i in 0..u64::from(opts.seconds * opts.fps) {
        let bytes = Bytes::from(vec![0u8; 64]);
        let s = Mp4Sample {
            start_time: i * 1000,
            duration: 1000,
            rendering_offset: 0,
            is_sync: i == 0,
            bytes,
        };
        w.write_sample(1, &s).unwrap();
    }
    if opts.audio {
        // One sample per second, `sample_rate` ticks each.
        for i in 0..u64::from(opts.seconds) {
            let bytes = Bytes::from(vec![0u8; 32]);
            let start_time = i * u64::from(opts.sample_rate);
            let s = Mp4Sample {
                start_time,
                duration: opts.sample_rate,
                rendering_offset: 0,
                is_sync: true,
                bytes,
            };
            w.write_sample(2, &s).unwrap();
        }
    }
    w.write_end().unwrap();
}
