//! Writes a tiny MP4 fixture at test time: 2 s, 25 fps H.264-tagged video + 48 kHz AAC audio.
#![allow(dead_code)]

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use bytes::Bytes;
use mp4::{
    AacConfig, AvcConfig, ChannelConfig, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer,
    SampleFreqIndex, TrackConfig, TrackType,
};

pub fn write_fixture(path: &Path) {
    let file = BufWriter::new(File::create(path).unwrap());
    let config = Mp4Config {
        major_brand: "isom".parse().unwrap(),
        minor_version: 512,
        compatible_brands: vec!["isom".parse().unwrap(), "mp41".parse().unwrap()],
        timescale: 1000,
    };
    let mut w = Mp4Writer::write_start(file, &config).unwrap();
    let video = AvcConfig {
        width: 320,
        height: 180,
        seq_param_set: vec![0x67, 0x42, 0x00, 0x1e, 0x95, 0xa8],
        pic_param_set: vec![0x68, 0xce, 0x3c, 0x80],
    };
    let audio = AacConfig {
        bitrate: 128_000,
        profile: mp4::AudioObjectType::AacLowComplexity,
        freq_index: SampleFreqIndex::Freq48000,
        chan_conf: ChannelConfig::Stereo,
    };
    w.add_track(&TrackConfig {
        track_type: TrackType::Video,
        timescale: 1000,
        language: "und".into(),
        media_conf: MediaConfig::AvcConfig(video),
    })
    .unwrap();
    w.add_track(&TrackConfig {
        track_type: TrackType::Audio,
        timescale: 48_000,
        language: "und".into(),
        media_conf: MediaConfig::AacConfig(audio),
    })
    .unwrap();
    // 50 video samples of 40 ms = 2 s; 96 audio samples of 1000 ticks = 2 s at 48 kHz.
    for i in 0..50u64 {
        let s = Mp4Sample {
            start_time: i * 40,
            duration: 40,
            rendering_offset: 0,
            is_sync: i == 0,
            bytes: Bytes::from(vec![0u8; 64]),
        };
        w.write_sample(1, &s).unwrap();
    }
    for i in 0..96u64 {
        let s = Mp4Sample {
            start_time: i * 1000,
            duration: 1000,
            rendering_offset: 0,
            is_sync: true,
            bytes: Bytes::from(vec![0u8; 32]),
        };
        w.write_sample(2, &s).unwrap();
    }
    w.write_end().unwrap();
}
