//! The proxy's audio track: the encoded length is the source's (Decision 22), a mono source
//! stays one channel, unreadable audio gives none, cancel stops it, and the packets are muxed
//! beside the frames by time, again in full when the video restarts on another encoder.

use std::io::Write;
use std::sync::Arc;

use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_codecs::testing::fake::{EncodeBehaviour, FakeEncoderBackend};
use fin_codecs::{
    DecoderBackend, EncoderBackend, EncoderConfig, Gop, ProbeResult, RateControl, RawFrame,
};
use fin_container::{Demuxer, Limits};
use fin_core::Rational;
use fin_media::{ColourTags, Size, VideoCodec};

use super::*;
use crate::media::encode_pipeline::{Frames, Track, encode_with_audio};

/// A file holding `bytes`, read from the start.
fn file_of(bytes: &[u8]) -> File {
    let mut f = tempfile::tempfile().unwrap();
    f.write_all(bytes).unwrap();
    use std::io::Seek;
    f.rewind().unwrap();
    f
}

/// `frames` frames of a 440 Hz tone at `rate` on `channels` channels, as a WAV file.
fn wav_file(frames: usize, channels: u16, rate: u32) -> File {
    let pcm: Vec<f32> = (0..frames)
        .flat_map(|n| {
            let x = (0.25 * (std::f64::consts::TAU * 440.0 * n as f64 / rate as f64).sin()) as f32;
            std::iter::repeat_n(x, channels as usize)
        })
        .collect();
    file_of(&wav(&pcm, channels, rate, WavFormat::F32, None))
}

fn encoded(frames: usize, channels: u16, rate: u32) -> AudioPackets {
    encode(wav_file(frames, channels, rate), || Ok(()))
        .unwrap()
        .expect("an audio track")
}

#[test]
fn encodes_exactly_the_source_samples() {
    // 44.1 kHz in, 48 kHz out: `resampled_len` frames, not a multiple of the 960-sample frame.
    let a = encoded(50_017, 2, 44_100);
    let valid = fin_audio::resampled_len(50_017, 44_100, 48_000);
    assert_ne!(valid % 960, 0);
    assert_eq!(a.track.presented, Some(valid));
    assert_eq!((a.track.sample_rate, a.track.channels), (48_000, 2));
    assert_eq!(a.track.priming, a.result.priming);
    assert!(a.result.priming > 0, "libopus has a lookahead");
    assert_eq!(a.result.bitrate, ProxyAudio::STEREO_BITRATE);
    assert_eq!(a.result.codec, AudioCodec::Opus);
    let decoded = a.len() as u64 * a.frame as u64;
    assert!(decoded >= a.result.priming as u64 + valid);
    assert!(decoded - a.result.priming as u64 - valid < a.frame as u64);
    assert!((0..a.len()).all(|i| !a.packet(i).is_empty()));
}

#[test]
fn a_mono_source_stays_one_channel() {
    let a = encoded(9_600, 1, 48_000);
    assert_eq!(a.track.channels, 1);
    assert_eq!(a.result.bitrate, ProxyAudio::MONO_BITRATE);
    assert_eq!(a.track.presented, Some(9_600));
}

#[test]
fn unreadable_or_empty_audio_is_none() {
    // Not an audio container (UNSUPPORTED_CONTAINER).
    let none = encode(
        file_of(b"this is not media at all, just some bytes"),
        || Ok(()),
    );
    assert!(none.unwrap().is_none());
    // A WAV with no samples.
    assert!(encode(wav_file(0, 2, 48_000), || Ok(())).unwrap().is_none());
    // A WAV cut inside its header is corrupt: the item's error, not a silent video-only proxy.
    let mut bytes = wav(&[0.0; 64], 2, 48_000, WavFormat::Pcm16, None);
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    let e = encode(file_of(&bytes), || Ok(())).unwrap_err();
    assert!(!unreadable(&e), "{e:?}");
}

#[test]
fn cancel_stops_the_encode() {
    let e = encode(wav_file(48_000, 2, 48_000), || {
        Err(CoreError::new("JOB_CANCELLED", "cancelled"))
    })
    .unwrap_err();
    assert_eq!(e.code, "JOB_CANCELLED");
}

/// Frames for the pipeline: grey 64 × 64 pictures into one output file.
struct Src {
    dir: tempfile::TempDir,
    outputs: u32,
}

impl Src {
    fn path(&self) -> std::path::PathBuf {
        let root = fin_shm::JobDir::resolve(&self.dir.path().join("x"))
            .unwrap()
            .root;
        root.with_file_name("proxy.mp4")
    }
}

impl Frames for Src {
    fn frame(&mut self, _: u64) -> Result<RawFrame, CoreError> {
        let size = Size::new(64, 64);
        Ok(RawFrame::nv12(size, vec![16; 64 * 64], vec![128; 64 * 32]))
    }

    fn between(&mut self, _: u64) -> Result<(), CoreError> {
        Ok(())
    }

    fn output(&mut self) -> Result<File, CoreError> {
        self.outputs += 1;
        fin_shm::create_output(&self.path())
    }
}

#[test]
fn muxed_beside_the_frames_by_time_also_after_a_restart() {
    // 3 s at 30 fps; the first encoder hangs, so the output is made twice.
    let hung = FakeEncoderBackend::new("mf-hw", true, &[VideoCodec::H264])
        .behaving(EncodeBehaviour::HangAfter(5))
        .arc();
    let ms = FakeEncoderBackend::new("mf-ms", false, &[VideoCodec::H264]).arc();
    let caps = ProbeResult::new(
        Vec::<Arc<dyn DecoderBackend>>::new(),
        vec![hung as Arc<dyn EncoderBackend>, ms],
    );
    let cfg = EncoderConfig {
        codec: VideoCodec::H264,
        profile: None,
        size: Size::new(64, 64),
        // The shortest hang deadline the policy allows (2 s).
        rate: Rational::from_int(1000),
        gop: Gop {
            key_every_frames: 7,
            closed: true,
            intra_only: false,
        },
        rc: RateControl::Crf(30),
        purpose: EncodePurpose::Proxy,
        adapter: None,
        preset: Some(10),
        threads: 1,
        colour: ColourTags::BT709,
    };
    let track = Track {
        timescale: 30_000,
        tick: 1000,
        colour: ColourTags::BT709,
    };
    let audio = encoded(3 * 48_000 - 77, 2, 48_000);
    let mut src = Src {
        dir: tempfile::tempdir().unwrap(),
        outputs: 0,
    };
    let out = encode_with_audio(&caps, &cfg, &track, 90, &mut src, Some(&audio)).unwrap();
    assert_eq!((out.entry.name, src.outputs), ("mf-ms", 2));
    let d = Demuxer::open(
        fin_shm::checked_open(&src.path(), u64::MAX).unwrap(),
        &Limits::default(),
    )
    .unwrap();
    let info = d.info().audio.clone().unwrap();
    // Decision 22 read back: the edit's media time is the priming, the rest past the presented
    // length is padding.
    let decoded = audio.len() as u64 * audio.frame as u64;
    assert_eq!(info.priming, audio.result.priming);
    assert_eq!(
        info.padding as u64,
        decoded - audio.result.priming as u64 - (3 * 48_000 - 77)
    );
    let a = d.audio().unwrap();
    assert_eq!(
        a.sample_count(),
        audio.len(),
        "every packet once, after the restart too"
    );
    let v = &d.video().unwrap().table;
    assert_eq!(v.len(), 90);
    // Interleaved: some audio lies before the last frame's bytes, some after the first's.
    let first_v = v.frames().iter().map(|f| f.offset).min().unwrap();
    let last_v = v.frames().iter().map(|f| f.offset).max().unwrap();
    let audio_at: Vec<u64> = (0..a.sample_count())
        .map(|i| a.sample(i).unwrap().offset)
        .collect();
    assert!(
        audio_at.iter().any(|o| *o > first_v && *o < last_v),
        "audio between the frames"
    );
}
