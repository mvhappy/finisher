//! The proxy's audio track: the encoded length is the source's (Decision 22), a mono source
//! stays one channel, the packets are spilled to the scratch file (only their sizes held), a
//! failed decode is a skip with its code, the decoder's class is checked, cancel stops it, and
//! the packets are muxed beside the frames in time order, again in full when the video restarts
//! on another encoder.
// The scratch file is checked by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

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

/// A job directory (its parent canonical: the checked opens refuse a link at any segment) with
/// an `out/`, and the scratch path in it.
struct Scratch {
    _dir: tempfile::TempDir,
    job: JobDir,
    path: PathBuf,
}

fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let job = JobDir::resolve(&dir.path().join("j")).unwrap();
    std::fs::create_dir_all(job.output()).unwrap();
    let path = job.output().join("k.audio");
    Scratch {
        _dir: dir,
        job,
        path,
    }
}

fn run(file: File, s: &Scratch) -> Result<Outcome, CoreError> {
    encode(file, VendorClass::Software, &s.job, &s.path, |_| Ok(()))
}

fn encoded(frames: usize, channels: u16, rate: u32, s: &Scratch) -> AudioPackets {
    match run(wav_file(frames, channels, rate), s).unwrap() {
        Outcome::Track(a) => a,
        o => panic!("an audio track, got {o:?}"),
    }
}

#[test]
fn encodes_exactly_the_source_samples() {
    // 44.1 kHz in, 48 kHz out: `resampled_len` frames, not a multiple of the 960-sample frame.
    let s = scratch();
    let a = encoded(50_017, 2, 44_100, &s);
    let valid = fin_audio::resampled_len(50_017, 44_100, 48_000);
    assert_ne!(valid % 960, 0);
    assert_eq!(a.track.presented, Some(valid));
    assert_eq!((a.track.sample_rate, a.track.channels), (48_000, 2));
    assert_eq!(a.track.priming, a.result.priming);
    assert!(a.result.priming > 0, "libopus has a lookahead");
    assert_eq!(a.result.bitrate, ProxyAudio::STEREO_BITRATE);
    assert_eq!(a.result.codec, ProxyAudioCodec::Opus);
    let decoded = a.len() as u64 * a.frame as u64;
    assert!(decoded >= a.result.priming as u64 + valid);
    assert!(decoded - a.result.priming as u64 - valid < a.frame as u64);
    // Review of Task 20 (S2b): the packets are in the scratch file, back to back; memory holds
    // only their sizes.
    let on_disk = std::fs::metadata(&s.path).unwrap().len();
    assert_eq!(on_disk, a.sizes.iter().map(|n| *n as u64).sum::<u64>());
    let mut r = a.reader().unwrap();
    let mut read = 0u64;
    for i in 0..a.len() {
        let p = r.next(i).unwrap();
        assert!(!p.is_empty());
        read += p.len() as u64;
    }
    assert_eq!(read, on_disk);
    drop(r);
    a.remove();
    assert!(!s.path.exists());
}

#[test]
fn a_mono_source_stays_one_channel() {
    let s = scratch();
    let a = encoded(9_600, 1, 48_000, &s);
    assert_eq!(a.track.channels, 1);
    assert_eq!(a.result.bitrate, ProxyAudio::MONO_BITRATE);
    assert_eq!(a.track.presented, Some(9_600));
}

#[test]
fn a_failed_decode_is_a_skip_with_its_code() {
    let s = scratch();
    // Not an audio container.
    let o = run(file_of(b"this is not media at all, just some bytes"), &s).unwrap();
    assert!(
        matches!(&o, Outcome::Skipped(c) if c == "UNSUPPORTED_CONTAINER"),
        "{o:?}"
    );
    // A WAV with no samples: no track, nothing skipped.
    assert!(matches!(
        run(wav_file(0, 2, 48_000), &s).unwrap(),
        Outcome::Empty
    ));
    assert!(!s.path.exists(), "nothing left of an empty track");
    // A WAV whose `fmt ` chunk says it is one byte long: corrupt, skipped with that code.
    let mut bytes = wav(&[0.0; 64], 2, 48_000, WavFormat::Pcm16, None);
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    match run(file_of(&bytes), &s).unwrap() {
        Outcome::Skipped(c) => assert!(
            ["MEDIA_CORRUPT", "UNSUPPORTED_CONTAINER"].contains(&c.as_str()),
            "{c}"
        ),
        o => panic!("a skip, got {o:?}"),
    }
    assert!(!s.path.exists(), "the scratch file of a skip is removed");
}

#[test]
fn another_decoder_class_is_plan_mismatch() {
    let s = scratch();
    let e = encode(
        wav_file(4_800, 2, 48_000),
        VendorClass::Os,
        &s.job,
        &s.path,
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["reason"]),
        ("PLAN_MISMATCH", &"cacheKey".into())
    );
    assert!(!s.path.exists(), "refused before anything is written");
}

#[test]
fn cancel_stops_the_encode() {
    let s = scratch();
    let e = encode(
        wav_file(48_000, 2, 48_000),
        VendorClass::Software,
        &s.job,
        &s.path,
        |_| Err(CoreError::new("JOB_CANCELLED", "cancelled")),
    )
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
    let s = scratch();
    let audio = encoded(3 * 48_000 - 77, 2, 48_000, &s);
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
    // Each packet in order at k × frame, its size the encoder's.
    let mut r = audio.reader().unwrap();
    for k in 0..a.sample_count() {
        let s = a.sample(k).unwrap();
        assert_eq!(s.dts, k as i64 * audio.frame as i64, "packet {k}");
        assert_eq!(s.size as usize, r.next(k).unwrap().len(), "packet {k}");
    }
    // Interleaved by time (review of Task 20, O2): the frames stored before each audio packet
    // are the ones that start before it, to within the muxer's 1 s chunks.
    let mut video_at: Vec<u64> = v.frames().iter().map(|f| f.offset).collect();
    video_at.sort_unstable();
    for k in 0..a.sample_count() {
        let s = a.sample(k).unwrap();
        let before = video_at.partition_point(|o| *o < s.offset) as f64 / 30.0;
        let at = s.dts as f64 / 48_000.0;
        assert!(
            (before - at).abs() <= 1.0 + 1.0 / 30.0,
            "packet {k} at {at:.3} s lies after {before:.3} s of video"
        );
    }
}
