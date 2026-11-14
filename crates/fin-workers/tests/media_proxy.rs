//! `media.proxy` v1 (plan 6B-1 Task 14), driven over the envelope as a host drives it
//! (`fin-workers media`, cold): one proxy frame per presented original frame (Decision 7, read
//! back by frame id for CFR, VFR, edit-listed and rotated clips), tier sizes (Decision 6), the
//! GOP rules read back from the proxy's sync samples (Decision 8), HDR to SDR (Decision 9),
//! rotation kept out of the pixels, the cache key the worker recomputes (Decision 11), library
//! files, and the typed refusals. Needs a GPU adapter for the scale pass (`FIN_GPU`; skipped
//! with a notice without one, failed with `FIN_REQUIRE_GPU=1`).
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::fs::File;
use std::path::{Path, PathBuf};

use fin_codecs::fixture::{ClipSpec, pattern_clip, timing};
use fin_codecs::{FrameReader, ProbeResult};
use fin_container::{Demuxer, Limits};
use fin_core::Rational;
use fin_jobs::HostMsg;
use fin_media::backend::VendorClass;
use fin_media::cache::{EntryMeta, META_FILE};
use fin_media::colour::{ChromaSiting, Matrix, Primaries, Range, Transfer};
use fin_media::jobs::{
    MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile, ProxyCodec, ProxyLibraryFile,
    ProxyParams, ProxyResult, ProxySourceAudio, ProxyTier, RenderManifest, ToneMapped, proxy_key,
};
use fin_media::table_bin::{self, TABLE_FILE};
use fin_media::testing::read_id_y;
use fin_media::{ColourTags, Rotation, Size};
use media_support::{gpu_or_skip, worker_env};
use serde_json::{Value, json};
use support::{End, Job, Proc, Run, expect_of, no_calls, rel, spawn_with};

const PROXY_FILE: &str = "proxy.mp4";

fn original(p: &Path) -> OriginalFile {
    let e = serde_json::to_value(expect_of(p)).unwrap();
    OriginalFile {
        path: p.to_str().unwrap().into(),
        size: e["size"].as_u64().unwrap(),
        mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
        file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
    }
}

/// A clip written into a fresh directory.
struct Clip {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn clip(spec: &ClipSpec) -> Clip {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.mp4");
    std::fs::write(&path, pattern_clip(spec).unwrap()).unwrap();
    Clip { _dir: dir, path }
}

/// A job whose manifest names only asset `a1` (fingerprint `fp`) at `path`.
fn job_for(fp: &str, path: &Path) -> Job {
    let job = Job::new();
    let m = RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: Some(original(path)),
            proxies: Vec::new(),
        }],
        fonts: ManifestFonts::default(),
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    job
}

fn params(fp: &str, tier: ProxyTier) -> ProxyParams {
    params_with(fp, tier, NO_AUDIO)
}

const NO_AUDIO: ProxySourceAudio = ProxySourceAudio::None;
/// Opus (libopus) and, on Linux, AAC (symphonia): decoded by a `software` backend.
const SOFTWARE_AUDIO: ProxySourceAudio = ProxySourceAudio::Decoded(VendorClass::Software);

/// The params of a source whose audio the host planned as `audio`.
fn params_with(fp: &str, tier: ProxyTier, audio: ProxySourceAudio) -> ProxyParams {
    ProxyParams {
        fingerprint: fp.into(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        audio,
        key: proxy_key(fp, tier, ProxyCodec::Av1, VendorClass::Software, audio).to_string(),
        asset: Some("a1".into()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    }
}

fn run(job: &Job, p: &ProxyParams) -> Run {
    let mut w: Proc = spawn_with(&["media"], &worker_env());
    w.hello();
    let grants: &[&str] = if p.asset.is_some() {
        &["in/manifest.json", "out/"]
    } else {
        &["out/"]
    };
    let start = job.start("media.proxy", serde_json::to_value(p).unwrap(), grants);
    w.send(&HostMsg::Start(start));
    w.drive(no_calls)
}

/// The item's result, or its error code.
fn outcome(run: &Run, fp: &str) -> Result<ProxyResult, String> {
    assert!(matches!(run.end, End::Result(_)), "{:?}", run.end);
    for e in &run.events {
        match e {
            support::Ev::Item(i, Some(v)) if i == fp => {
                return Ok(serde_json::from_value(v.clone()).unwrap());
            }
            support::Ev::ItemFailed(i, code) if i == fp => return Err(code.clone()),
            _ => {}
        }
    }
    panic!("no outcome for {fp}: {:?}", run.events)
}

/// The proxy's directory, `out/<key>/`.
fn entry(job: &Job, key: &str) -> PathBuf {
    job.out().join(key)
}

fn meta(job: &Job, key: &str) -> EntryMeta {
    serde_json::from_slice(&std::fs::read(entry(job, key).join(META_FILE)).unwrap()).unwrap()
}

/// Every frame id of `path` in presentation order (decoded with dav1d).
fn ids(path: &Path) -> Vec<Option<u32>> {
    let mut r = FrameReader::open(
        File::open(path).unwrap(),
        &Limits::default(),
        &ProbeResult::software_only(),
    )
    .unwrap();
    (0..r.table().len())
        .map(|i| {
            let f = r.frame(i).unwrap();
            let p = &f.planes[0];
            let y: Vec<u8> = (0..f.size.height)
                .flat_map(|row| p.row(row, 1).to_vec())
                .collect();
            read_id_y(&y, f.size.width, f.size.height)
        })
        .collect()
}

fn demux(path: &Path) -> Demuxer<File> {
    Demuxer::open(File::open(path).unwrap(), &Limits::default()).unwrap()
}

/// Makes a proxy of `spec` at `tier`: the clip, the job and the result.
fn made(spec: &ClipSpec, tier: ProxyTier) -> (Clip, Job, ProxyResult) {
    let c = clip(spec);
    let job = job_for("fp", &c.path);
    let r = outcome(&run(&job, &params("fp", tier)), "fp").expect("a proxy");
    (c, job, r)
}

fn spec(frames: u32, size: Size) -> ClipSpec {
    ClipSpec {
        frames,
        size,
        keyint: 10,
        ..ClipSpec::default()
    }
}

#[test]
fn proxy_frames_match_original_ids() {
    if !gpu_or_skip("proxy_frames_match_original_ids") {
        return;
    }
    let (n, d) = timing(Rational::from_int(30));
    let tick = d as u64;
    let _ = n;
    let cases = [
        ("cfr", spec(24, Size::new(1280, 720))),
        (
            "vfr",
            ClipSpec {
                vfr: true,
                ..spec(20, Size::new(320, 180))
            },
        ),
        (
            // Frames 5–14, then 0–9 again: one media frame presented twice.
            "edits",
            ClipSpec {
                edits: Some(vec![(10 * tick, Some(5 * tick)), (10 * tick, Some(0))]),
                ..spec(20, Size::new(320, 180))
            },
        ),
        (
            "rotated",
            ClipSpec {
                rotation: Rotation::R90,
                ..spec(16, Size::new(320, 180))
            },
        ),
    ];
    for (name, s) in cases {
        let (c, job, r) = made(&s, ProxyTier::Scrub);
        let want = ids(&c.path);
        let table = demux(&c.path).video().unwrap().table.clone();
        assert_eq!(want.len(), table.len(), "{name}");
        assert!(want.iter().all(Option::is_some), "{name}: {want:?}");
        let dir = entry(&job, &r.key);
        let got = ids(&dir.join(PROXY_FILE));
        assert_eq!(got, want, "{name}: proxy frame i shows original frame i");
        assert_eq!(r.frames, table.len() as u64, "{name}");
        // CFR at the original's nominal rate, one sample per presented frame.
        let proxy = demux(&dir.join(PROXY_FILE));
        let pv = proxy.video().unwrap();
        assert_eq!(pv.table.len(), table.len(), "{name}");
        let info = proxy.info().video.clone().unwrap();
        assert!(!info.vfr, "{name}: the proxy is CFR");
        assert_eq!(info.nominal_rate, Rational::from_int(30), "{name}");
        // `table.bin` is the original's sample table.
        let bin = std::fs::read(dir.join(TABLE_FILE)).unwrap();
        assert_eq!(table_bin::decode(&bin).unwrap(), table, "{name}");
        // `meta.json` lists the files and keeps the original's MediaInfo.
        let m = meta(&job, &r.key);
        m.check().unwrap();
        assert_eq!(m.media.as_ref(), Some(demux(&c.path).info()), "{name}");
        let names: Vec<_> = m.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [PROXY_FILE, TABLE_FILE], "{name}");
        assert_eq!(r.bytes, m.bytes().unwrap(), "{name}");
    }
}

#[test]
fn tier_sizes_and_no_upscale() {
    if !gpu_or_skip("tier_sizes_and_no_upscale") {
        return;
    }
    let s = spec(4, Size::new(1280, 720));
    for (tier, want) in [
        (ProxyTier::Scrub, Size::new(960, 540)),
        // Never upscaled: 1280 × 720 stays its size for the 1080 tier.
        (ProxyTier::Preview, Size::new(1280, 720)),
    ] {
        let (_c, job, r) = made(&s, tier);
        assert_eq!(r.size, want, "{tier:?}");
        let proxy = demux(&entry(&job, &r.key).join(PROXY_FILE));
        let v = proxy.info().video.clone().unwrap();
        assert_eq!(v.coded, want, "{tier:?}");
        assert_eq!(meta(&job, &r.key).recipe, tier.recipe().name);
    }
}

/// Indices of the proxy's keyframes, after checking it has no reordering (B-frames).
fn keyframes(path: &Path) -> Vec<usize> {
    let d = demux(path);
    let t = &d.video().unwrap().table;
    for (i, f) in t.frames().iter().enumerate() {
        assert_eq!(
            f.decode_index as usize, i,
            "no B-frames: decode order is presentation order"
        );
    }
    (t.frames().iter().enumerate())
        .filter(|(_, f)| f.keyframe)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn gop_rules_540_and_1080() {
    if !gpu_or_skip("gop_rules_540_and_1080") {
        return;
    }
    // 30 fps: 540 a keyframe every 7 frames (≤ 0.25 s, no intra-only measurement: Task 8);
    // 1080 every 15 (≤ 0.5 s, SVT-AV1's closed GOP of 15).
    let s = spec(40, Size::new(320, 180));
    for (tier, every) in [(ProxyTier::Scrub, 7), (ProxyTier::Preview, 15)] {
        let (_c, job, r) = made(&s, tier);
        let keys = keyframes(&entry(&job, &r.key).join(PROXY_FILE));
        let want: Vec<usize> = (0..40).step_by(every).collect();
        assert_eq!(keys, want, "{tier:?}");
    }
}

#[test]
fn hdr_source_gives_sdr_proxy() {
    if !gpu_or_skip("hdr_source_gives_sdr_proxy") {
        return;
    }
    let pq = ColourTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
        siting: ChromaSiting::Left,
    };
    let s = ClipSpec {
        colour: pq,
        ..spec(6, Size::new(320, 180))
    };
    let (c, job, r) = made(&s, ProxyTier::Scrub);
    assert_eq!(r.tone_mapped, Some(ToneMapped::Shader));
    let m = meta(&job, &r.key);
    assert_eq!(m.tone_mapped, Some(ToneMapped::Shader));
    // The original's HDR tags are kept in the entry; the proxy is BT.709 SDR.
    assert_eq!(
        m.media.unwrap().video.unwrap().colour.transfer,
        Transfer::Pq
    );
    let path = entry(&job, &r.key).join(PROXY_FILE);
    let v = demux(&path).info().video.clone().unwrap();
    // `colr` (nclx) carries no siting: compare the rest.
    let tags = |c: ColourTags| (c.primaries, c.transfer, c.matrix, c.range);
    assert_eq!(tags(v.colour), tags(ColourTags::BT709));
    assert!(v.hdr.is_none());
    assert_eq!(ids(&path), ids(&c.path));
    // An SDR source is not tone-mapped.
    let (_c, _job, sdr) = made(&spec(2, Size::new(320, 180)), ProxyTier::Scrub);
    assert_eq!(sdr.tone_mapped, None);
}

#[test]
fn rotation_not_baked() {
    if !gpu_or_skip("rotation_not_baked") {
        return;
    }
    let s = ClipSpec {
        rotation: Rotation::R90,
        ..spec(3, Size::new(320, 180))
    };
    let (_c, job, r) = made(&s, ProxyTier::Scrub);
    // The coded orientation and size ratio: 320 × 180, not 180 × 320.
    assert_eq!(r.size, Size::new(320, 180));
    let v = demux(&entry(&job, &r.key).join(PROXY_FILE))
        .info()
        .video
        .clone()
        .unwrap();
    assert_eq!((v.coded, v.rotation), (Size::new(320, 180), Rotation::R0));
    // The original's rotation is in the entry, for the compositor to apply.
    let m = meta(&job, &r.key).media.unwrap().video.unwrap();
    assert_eq!(m.rotation, Rotation::R90);
}

#[test]
fn cache_key_mismatch_is_plan_mismatch() {
    let c = clip(&spec(2, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    // The host expected an OS decoder; dav1d (software) decodes it here.
    let mut p = params("fp", ProxyTier::Scrub);
    p.decoder_class = VendorClass::Os;
    p.key = proxy_key(
        "fp",
        ProxyTier::Scrub,
        ProxyCodec::Av1,
        VendorClass::Os,
        NO_AUDIO,
    )
    .to_string();
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
    // A key that is not the params' at all.
    let mut p = params("fp", ProxyTier::Scrub);
    p.key = proxy_key(
        "other",
        ProxyTier::Scrub,
        ProxyCodec::Av1,
        VendorClass::Software,
        NO_AUDIO,
    )
    .to_string();
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
    // Nothing was written for either.
    assert_eq!(std::fs::read_dir(job.out()).unwrap().count(), 0);
}

#[test]
fn h264_without_a_hardware_encoder_is_codec_unavailable() {
    // Linux has no H.264 encoder until 6B-2 (D13: Finisher ships none of its own).
    if cfg!(not(target_os = "linux")) {
        return;
    }
    let c = clip(&spec(2, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    let mut p = params("fp", ProxyTier::Scrub);
    p.codec = ProxyCodec::H264;
    p.key = proxy_key(
        "fp",
        ProxyTier::Scrub,
        ProxyCodec::H264,
        VendorClass::Software,
        NO_AUDIO,
    )
    .to_string();
    assert_eq!(
        outcome(&run(&job, &p), "fp").unwrap_err(),
        "CODEC_UNAVAILABLE"
    );
}

#[test]
fn a_changed_asset_or_file_is_refused() {
    let c = clip(&spec(2, Size::new(320, 180)));
    // The manifest's fingerprint is not the one asked for.
    let job = job_for("fp-new", &c.path);
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "ASSET_CHANGED"
    );
    // The file changed after the manifest was written.
    let job = job_for("fp", &c.path);
    std::fs::write(&c.path, b"something else").unwrap();
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "ORIGINALS_MISSING"
    );
}

fn library_params(c: &Clip, size_off: u64) -> ProxyParams {
    let root = c.path.parent().unwrap();
    let mut expect = serde_json::to_value(expect_of(&c.path)).unwrap();
    expect["size"] = json!(expect["size"].as_u64().unwrap() + size_off);
    ProxyParams {
        asset: None,
        file: Some(ProxyLibraryFile {
            library: "l1".into(),
            root: root.to_str().unwrap().into(),
            rel: serde_json::to_value(rel(&["clip.mp4"])).unwrap(),
            expect,
            hydrate: false,
        }),
        ..params("fp", ProxyTier::Scrub)
    }
}

#[test]
fn library_files_open_through_the_catalogue_s_expectations() {
    let c = clip(&spec(3, Size::new(320, 180)));
    let job = Job::new();
    // A file that is not what the catalogue says is FILE_CHANGED (the stage rescans it).
    assert_eq!(
        outcome(&run(&job, &library_params(&c, 1)), "fp").unwrap_err(),
        "FILE_CHANGED"
    );
    if !gpu_or_skip("library_files_open_through_the_catalogue_s_expectations") {
        return;
    }
    let r = outcome(&run(&job, &library_params(&c, 0)), "fp").unwrap();
    assert_eq!(r.frames, 3);
    assert_eq!(ids(&entry(&job, &r.key).join(PROXY_FILE)), ids(&c.path));
}

#[test]
fn progress_counts_frames() {
    if !gpu_or_skip("progress_counts_frames") {
        return;
    }
    let c = clip(&spec(12, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    let run = run(&job, &params("fp", ProxyTier::Scrub));
    outcome(&run, "fp").unwrap();
    let p = run.progress();
    assert!(!p.is_empty());
    assert!(p.windows(2).all(|w| w[0] <= w[1]), "{p:?}");
    assert_eq!(p.last().copied(), Some(12.0));
}

#[test]
fn dolby_vision_profile_5_is_codec_unavailable() {
    use fin_media::{DolbyVision, HdrInfo};
    let dv = |profile| HdrInfo {
        mastering: None,
        content_light: None,
        dolby_vision: Some(DolbyVision {
            profile,
            level: 6,
            rpu: true,
            enhancement_layer: false,
            base_layer: true,
            compatibility: if profile == 5 { 0 } else { 1 },
        }),
    };
    let c = clip(&ClipSpec {
        hdr: Some(dv(5)),
        ..spec(2, Size::new(320, 180))
    });
    let job = job_for("fp", &c.path);
    assert_eq!(
        outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap_err(),
        "CODEC_UNAVAILABLE"
    );
    // Profile 8 proxies its base layer.
    if !gpu_or_skip("dolby_vision_profile_5_is_codec_unavailable") {
        return;
    }
    let c = clip(&ClipSpec {
        hdr: Some(dv(8)),
        ..spec(2, Size::new(320, 180))
    });
    let job = job_for("fp", &c.path);
    outcome(&run(&job, &params("fp", ProxyTier::Scrub)), "fp").unwrap();
}

/// Review of Task 14: an output the worker cannot write names it relative to the job directory;
/// no absolute path of the host reaches the job's error (each output file in turn).
#[test]
fn refused_outputs_name_no_absolute_path() {
    if !gpu_or_skip("refused_outputs_name_no_absolute_path") {
        return;
    }
    let c = clip(&spec(2, Size::new(320, 180)));
    for blocked in [PROXY_FILE, TABLE_FILE, META_FILE] {
        let job = job_for("fp", &c.path);
        let p = params("fp", ProxyTier::Scrub);
        // A directory (not empty) where the output file goes: it cannot be made.
        let dir = entry(&job, &p.key).join(blocked);
        std::fs::create_dir_all(dir.join("x")).unwrap();
        let run = run(&job, &p);
        let root = std::fs::canonicalize(job.dir.path()).unwrap();
        let text = format!("{:?} {:?}", run.end, run.events);
        assert!(
            matches!(run.end, End::Error(_)) || text.contains("ItemFailed"),
            "{blocked}: {text}"
        );
        for p in [root.as_path(), job.dir.path()] {
            assert!(!text.contains(&*p.to_string_lossy()), "{blocked}: {text}");
        }
    }
}

/// Review of Task 14: a source that presents no frame is `MEDIA_CORRUPT`, not an empty proxy.
#[test]
fn a_source_without_frames_is_media_corrupt() {
    let c = clip(&spec(0, Size::new(320, 180)));
    let job = job_for("fp", &c.path);
    let p = params("fp", ProxyTier::Scrub);
    assert_eq!(outcome(&run(&job, &p), "fp").unwrap_err(), "MEDIA_CORRUPT");
    assert!(!entry(&job, &p.key).join(META_FILE).exists());
}

// Proxy recipe v2 (6C plan Task 20; spec §7.1): Opus audio inside the proxy.

use fin_audio::testing::analyse::rms_dbfs;
use fin_audio::testing::signals::{sweep, tone_burst};
use fin_codecs::audio::{AudioEncoderConfig, AudioLimits, AudioReader, open_audio_encoder};
use fin_container::{AudioTrackConfig, CodecConfig, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_media::AudioCodec;
use fin_media::backend::EncodePurpose;
use fin_media::cache::{KeyInput, PROXY_540, PROXY_1080};
use fin_media::jobs::{ProxyAudio, ProxyAudioCodec, ProxyAudioSkipped};

/// The audio of a test clip: interleaved `f32` at 48 kHz, its channel count, and the codec
/// it is stored in (`Other` packets are not audio: no decoder reads them). `corrupt` replaces
/// every packet after the first few with bytes no decoder accepts.
struct AudioSpec {
    pcm: Vec<f32>,
    channels: u16,
    codec: AudioCodec,
    corrupt: bool,
}

impl AudioSpec {
    fn new(pcm: Vec<f32>, channels: u16, codec: AudioCodec) -> AudioSpec {
        AudioSpec {
            pcm,
            channels,
            codec,
            corrupt: false,
        }
    }
}

/// `pcm` encoded with this build's encoder for `codec` (a test fixture), as an MP4 audio
/// track's configuration, priming, presented length and packets of `frame` samples.
fn encoded_audio(a: &AudioSpec) -> (AudioTrackConfig, u32, Vec<Vec<u8>>) {
    if a.codec == AudioCodec::Other {
        let frames = a.pcm.len() / a.channels as usize;
        let packets = vec![vec![0x5a; 64]; frames.div_ceil(1536).max(1)];
        let cfg = AudioTrackConfig {
            config: CodecConfig::Other { fourcc: *b"ac-3" },
            sample_rate: 48_000,
            channels: a.channels,
            priming: 0,
            presented: Some(frames as u64),
        };
        return (cfg, 1536, packets);
    }
    let cfg = AudioEncoderConfig {
        codec: a.codec,
        rate: 48_000,
        channels: a.channels,
        bitrate: 64_000 * a.channels as u32,
    };
    let mut enc = open_audio_encoder(&cfg, EncodePurpose::TestFixture).unwrap();
    enc.send(&a.pcm, 0).unwrap();
    let len = enc.finish().unwrap();
    let mut packets = Vec::new();
    while let Some(p) = enc.receive().unwrap() {
        packets.push(p.data);
    }
    if a.corrupt {
        // A TOC byte of code 3 with a frame count of 0: refused by libopus.
        for p in packets.iter_mut().skip(3) {
            *p = vec![0xFF, 0x00];
        }
    }
    let track = AudioTrackConfig {
        config: enc.config(),
        sample_rate: 48_000,
        channels: a.channels,
        priming: len.priming,
        presented: Some(len.valid),
    };
    (track, enc.frame_size(), packets)
}

/// A pattern clip (frame `i` carries id `i`) with `audio` beside it, as one MP4.
fn av_clip(spec: &ClipSpec, audio: &AudioSpec) -> Clip {
    use fin_codecs::fixture::{encode_all, fixture_config};
    use fin_codecs::{RawFrame, open_encoder};
    let mut enc = open_encoder(
        &fixture_config(spec, fin_media::VideoCodec::Av1),
        &ProbeResult::software_only(),
    )
    .unwrap();
    let packets = encode_all(&mut enc, spec.frames, |i| {
        let p = fin_media::testing::pattern(i, spec.size.width, spec.size.height, spec.seed);
        RawFrame::i420(spec.size, p.y, p.u, p.v)
    })
    .unwrap();
    let (timescale, tick) = timing(spec.rate);
    let mut w = Mp4Writer::new(std::io::Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let v = w
        .add_video(VideoTrackConfig {
            config: enc.config().unwrap(),
            coded: spec.size,
            timescale,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    let (cfg, frame, audio_packets) = encoded_audio(audio);
    let a = w.add_audio(cfg).unwrap();
    for pk in &packets {
        let at = pk.pts * tick as i64;
        w.write(v, &pk.data, at, at, tick, pk.keyframe).unwrap();
    }
    for (k, p) in audio_packets.iter().enumerate() {
        let at = k as i64 * frame as i64;
        w.write(a, p, at, at, frame, true).unwrap();
    }
    let bytes = w.finish().unwrap().into_inner();
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.mp4");
    std::fs::write(&path, bytes).unwrap();
    Clip { _dir: dir, path }
}

/// `path`'s audio, decoded and trimmed (priming and padding): interleaved, and its channels.
fn decoded_audio(path: &Path) -> (Vec<f32>, usize) {
    let mut r = AudioReader::open(File::open(path).unwrap(), &AudioLimits::default()).unwrap();
    let ch = r.info().channels as usize;
    let mut out = Vec::new();
    while let Some(d) = r.next().unwrap() {
        out.extend_from_slice(&d.interleaved);
    }
    (out, ch)
}

/// `n` frames of a 60 Hz–12 kHz sweep at −12 dBFS (no period: a lag has one best match) on
/// `channels` identical channels.
fn tone(n: usize, channels: u16) -> Vec<f32> {
    let s = sweep(60.0, 12_000.0, (n + 48) as f64 / 48_000.0, 48_000);
    s[..n]
        .iter()
        .flat_map(|x| std::iter::repeat_n(*x * 0.25, channels as usize))
        .collect()
}

/// The lag in `-max..=max` at which `got` (channel 0 of `gch`) best matches `want` (channel 0
/// of `wch`): normalised cross-correlation over the middle of the signal.
fn best_lag(got: &[f32], gch: usize, want: &[f32], wch: usize, max: i64) -> i64 {
    let g: Vec<f64> = got.iter().step_by(gch).map(|x| *x as f64).collect();
    let w: Vec<f64> = want.iter().step_by(wch).map(|x| *x as f64).collect();
    let (from, to) = (max as usize * 2, g.len().min(w.len()) - max as usize * 2);
    let score = |lag: i64| {
        let (mut dot, mut gg, mut ww) = (0.0, 0.0, 0.0);
        for i in from..to {
            let (a, b) = (g[(i as i64 + lag) as usize], w[i]);
            dot += a * b;
            gg += a * a;
            ww += b * b;
        }
        dot / (gg * ww).sqrt()
    };
    (-max..=max)
        .max_by(|a, b| score(*a).total_cmp(&score(*b)))
        .unwrap()
}

#[test]
fn proxy_v2_has_opus_audio() {
    if !gpu_or_skip("proxy_v2_has_opus_audio") {
        return;
    }
    // Lengths that are not a multiple of Opus's 960-sample frame.
    let mut cases = vec![
        ("opus stereo", AudioCodec::Opus, 2u16, 30_017usize),
        ("opus mono", AudioCodec::Opus, 1, 29_999),
    ];
    // AAC decodes on Linux only (symphonia; the OS decoders arrive with 6C Tasks 15-16).
    if cfg!(target_os = "linux") {
        cases.push(("aac stereo", AudioCodec::Aac, 2, 31_234));
    }
    for (name, codec, channels, n) in cases {
        let c = av_clip(
            &spec(20, Size::new(320, 180)),
            &AudioSpec::new(tone(n, channels), channels, codec),
        );
        let job = job_for("fp", &c.path);
        let p = params_with("fp", ProxyTier::Scrub, SOFTWARE_AUDIO);
        let r = outcome(&run(&job, &p), "fp").expect(name);
        let path = entry(&job, &r.key).join(PROXY_FILE);
        // The video is untouched by the audio track: proxy frame i shows original frame i.
        assert_eq!(ids(&path), ids(&c.path), "{name}");
        let info = demux(&path).info().audio.clone().expect(name);
        assert_eq!(info.codec, AudioCodec::Opus, "{name}");
        assert_eq!(info.sample_rate, 48_000, "{name}");
        // A mono source stays one channel (6C plan Decision 3), at half the stereo bitrate.
        assert_eq!(info.channels, channels, "{name}");
        let bitrate = if channels == 1 {
            ProxyAudio::MONO_BITRATE
        } else {
            ProxyAudio::STEREO_BITRATE
        };
        assert_eq!(
            r.audio,
            Some(ProxyAudio {
                codec: ProxyAudioCodec::Opus,
                bitrate,
                priming: info.priming,
            }),
            "{name}"
        );
        assert_eq!(r.audio_skipped, None, "{name}");
        // Exactly the source's samples after the priming (± 0), its level within 0.3 dB, and
        // aligned to the sample (the best-matching lag is 0: a priming off by a few samples
        // moves it).
        let (want, wch) = decoded_audio(&c.path);
        let (got, gch) = decoded_audio(&path);
        assert_eq!((gch, wch), (channels as usize, channels as usize), "{name}");
        assert_eq!(got.len() / gch, want.len() / wch, "{name}: frames");
        assert_eq!(
            want.len() / wch,
            n,
            "{name}: the source decodes to its frames"
        );
        let (a, b) = (rms_dbfs(&got), rms_dbfs(&want));
        assert!((a - b).abs() <= 0.3, "{name}: {a:.3} dBFS vs {b:.3} dBFS");
        assert_eq!(best_lag(&got, gch, &want, wch, 96), 0, "{name}: lag");
        let m = meta(&job, &r.key);
        m.check().unwrap();
        assert_eq!(m.recipe_version, 2, "{name}");
        assert_eq!(
            m.params["audio"],
            json!({ "decoded": "software" }),
            "{name}"
        );
        // Nothing but the entry is left in `out/` (the packets' scratch file is gone).
        let left: Vec<_> = std::fs::read_dir(job.out())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from(&r.key)], "{name}");
    }
}

#[test]
fn proxy_without_audio_stays_video_only() {
    if !gpu_or_skip("proxy_without_audio_stays_video_only") {
        return;
    }
    let (_c, job, r) = made(&spec(6, Size::new(320, 180)), ProxyTier::Scrub);
    assert_eq!((r.audio, r.audio_skipped), (None, None));
    let path = entry(&job, &r.key).join(PROXY_FILE);
    assert!(demux(&path).info().audio.is_none());
    assert_eq!(meta(&job, &r.key).params["audio"], json!("none"));
    // An audio track nothing here decodes (AC-3) also gives a video-only proxy, which says
    // why; the key names the track as unavailable, so a build that decodes it makes another.
    let c = av_clip(
        &spec(6, Size::new(320, 180)),
        &AudioSpec::new(tone(9_600, 2), 2, AudioCodec::Other),
    );
    let job = job_for("fp", &c.path);
    let p = params_with("fp", ProxyTier::Scrub, ProxySourceAudio::Unavailable);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.audio, None);
    assert_eq!(
        r.audio_skipped,
        Some(ProxyAudioSkipped {
            code: "CODEC_UNAVAILABLE".into()
        })
    );
    let path = entry(&job, &r.key).join(PROXY_FILE);
    assert!(demux(&path).info().audio.is_none());
    assert_eq!(ids(&path), ids(&c.path));
}

/// Review of Task 20 (O1): a corrupt audio track does not cost the preview its frames: the proxy
/// is video only and says why (the mix reads `pcm48`, which fails on its own); the key still
/// names the track as decodable.
#[test]
fn corrupt_audio_gives_a_video_only_proxy() {
    if !gpu_or_skip("corrupt_audio_gives_a_video_only_proxy") {
        return;
    }
    let c = av_clip(
        &spec(6, Size::new(320, 180)),
        &AudioSpec {
            corrupt: true,
            ..AudioSpec::new(tone(48_000, 2), 2, AudioCodec::Opus)
        },
    );
    let job = job_for("fp", &c.path);
    let r = outcome(
        &run(&job, &params_with("fp", ProxyTier::Scrub, SOFTWARE_AUDIO)),
        "fp",
    )
    .expect("a video-only proxy");
    assert_eq!(r.audio, None);
    let code = r.audio_skipped.expect("audioSkipped").code;
    assert!(
        ["DECODE_FAILED", "MEDIA_CORRUPT"].contains(&code.as_str()),
        "{code}"
    );
    let path = entry(&job, &r.key).join(PROXY_FILE);
    assert!(demux(&path).info().audio.is_none());
    assert_eq!(ids(&path), ids(&c.path));
    let left = std::fs::read_dir(job.out()).unwrap().count();
    assert_eq!(left, 1, "no scratch file left behind");
}

/// Review of Task 20 (S1): the worker recomputes the audio half of the key from the file and
/// refuses a plan that said otherwise, before anything is written.
#[test]
fn an_audio_plan_mismatch_is_refused() {
    let c = av_clip(
        &spec(2, Size::new(320, 180)),
        &AudioSpec::new(tone(4_800, 2), 2, AudioCodec::Opus),
    );
    let job = job_for("fp", &c.path);
    for planned in [NO_AUDIO, ProxySourceAudio::Unavailable] {
        let p = params_with("fp", ProxyTier::Scrub, planned);
        assert_eq!(
            outcome(&run(&job, &p), "fp").unwrap_err(),
            "PLAN_MISMATCH",
            "{planned:?}"
        );
    }
    // A video-only source planned as having audio.
    let v = clip(&spec(2, Size::new(320, 180)));
    let job2 = job_for("fp", &v.path);
    let p = params_with("fp", ProxyTier::Scrub, SOFTWARE_AUDIO);
    assert_eq!(outcome(&run(&job2, &p), "fp").unwrap_err(), "PLAN_MISMATCH");
    for j in [&job, &job2] {
        assert_eq!(std::fs::read_dir(j.out()).unwrap().count(), 0);
    }
}

/// Review of Task 20 (S2a): the audio pass, which runs before the first frame, reports progress
/// (phase `encodeAudio`) so a long source is not stalled; the stream stays monotonic.
#[test]
fn the_audio_pass_reports_progress() {
    if !gpu_or_skip("the_audio_pass_reports_progress") {
        return;
    }
    let c = av_clip(
        &spec(12, Size::new(320, 180)),
        &AudioSpec::new(tone(24_000, 2), 2, AudioCodec::Opus),
    );
    let job = job_for("fp", &c.path);
    let run = run(&job, &params_with("fp", ProxyTier::Scrub, SOFTWARE_AUDIO));
    outcome(&run, "fp").unwrap();
    let phases: Vec<_> = run.phases.iter().map(|p| p.as_deref()).collect();
    let first_encode = phases.iter().position(|p| *p == Some("encode")).unwrap();
    assert!(
        phases[..first_encode].contains(&Some("encodeAudio")),
        "{phases:?}"
    );
    let p = run.progress();
    assert!(p.windows(2).all(|w| w[0] <= w[1]), "{p:?}");
    assert_eq!(p.last().copied(), Some(12.0));
}

#[test]
fn proxy_v1_keys_invalidated() {
    // Recipe v2 carries audio, so a v1 (video-only) entry of the same source is another key.
    assert_eq!((PROXY_540.version, PROXY_1080.version), (2, 2));
    for tier in ProxyTier::ALL {
        let v2 = proxy_key("fp", tier, ProxyCodec::Av1, VendorClass::Software, NO_AUDIO);
        let mut v1: KeyInput = fin_media::jobs::key_input(
            "fp",
            tier,
            ProxyCodec::Av1,
            VendorClass::Software,
            NO_AUDIO,
        );
        assert_eq!(v1.recipe_version, 2, "{tier:?}");
        v1.recipe_version = 1;
        assert_ne!(
            fin_media::cache::CacheKey::compute(&v1),
            v2,
            "{tier:?}: same source, new key"
        );
    }
}

#[test]
fn proxy_audio_starts_with_video() {
    if !gpu_or_skip("proxy_audio_starts_with_video") {
        return;
    }
    // A beep at frame 12's time (0.4 s at 30 fps) for 0.1 s, in 1 s of silence.
    let flash = 12u32;
    let burst = tone_burst(1_000.0, -6.0, flash as f64 / 30.0, 0.1, 1.0, 48_000);
    let pcm: Vec<f32> = burst.iter().flat_map(|x| [*x, *x]).collect();
    let c = av_clip(
        &spec(30, Size::new(320, 180)),
        &AudioSpec::new(pcm, 2, AudioCodec::Opus),
    );
    let job = job_for("fp", &c.path);
    let p = params_with("fp", ProxyTier::Scrub, SOFTWARE_AUDIO);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    let path = entry(&job, &r.key).join(PROXY_FILE);
    // The proxy frame showing the flash frame's id, at its presentation time in the proxy
    // (the track's edited table, in its timescale).
    let at = ids(&path)
        .iter()
        .position(|id| *id == Some(flash))
        .expect("the flash frame");
    let d = demux(&path);
    let table = &d.video().unwrap().table;
    let video_s = table.frames()[at].pts as f64 / table.timescale() as f64;
    // The beep's onset: the first sample above half its amplitude, the audio's presented
    // sample 0 being movie time 0 (the edit's media time is the priming).
    let (pcm, ch) = decoded_audio(&path);
    let half = 0.5 * fin_audio::testing::signals::db_to_amp(-6.0) as f32;
    let onset = pcm
        .chunks(ch)
        .position(|f| f[0].abs() > half)
        .expect("the beep") as f64;
    let audio_s = onset / 48_000.0;
    assert!(
        (audio_s - video_s).abs() <= 0.001,
        "audio at {audio_s:.5} s, video at {video_s:.5} s"
    );
}

/// A pattern clip in WebM (AV1, 30 fps) with an Opus audio track beside it: this build reads no
/// WebM audio, so the proxy key names it unavailable.
fn webm_av_clip(spec: &ClipSpec) -> Clip {
    use fin_codecs::fixture::{encode_all, fixture_config};
    use fin_codecs::{RawFrame, open_encoder};
    use fin_container::testing::webm_writer::{Block, Options, Track, write};
    let mut enc = open_encoder(
        &fixture_config(spec, fin_media::VideoCodec::Av1),
        &ProbeResult::software_only(),
    )
    .unwrap();
    let packets = encode_all(&mut enc, spec.frames, |i| {
        let p = fin_media::testing::pattern(i, spec.size.width, spec.size.height, spec.seed);
        RawFrame::i420(spec.size, p.y, p.u, p.v)
    })
    .unwrap();
    let CodecConfig::Av1 { av1c } = enc.config().unwrap() else {
        panic!("not an av1C")
    };
    // OpusHead: version 1, 2 channels, pre-skip 312, 48 kHz, gain 0, mapping family 0.
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 0]);
    let mut blocks: Vec<Block> = packets
        .iter()
        .map(|pk| Block::simple(1, (pk.pts * 1000 + 15) / 30, pk.keyframe, pk.data.clone()))
        .collect();
    blocks.extend((0..10).map(|i| Block::simple(2, i * 20, true, vec![0xFC, 0xFF, 0xFE])));
    blocks.sort_by_key(|b| b.ts);
    let (w, h) = (spec.size.width as u64, spec.size.height as u64);
    let bytes = write(
        &Options::default(),
        &[
            Track {
                private: Some(av1c),
                ..Track::video(1, "V_AV1", w, h)
            },
            Track {
                private: Some(head),
                ..Track::audio(2, "A_OPUS", 48_000.0, 2)
            },
        ],
        &blocks,
    );
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap().join("clip.webm");
    std::fs::write(&path, bytes).unwrap();
    Clip { _dir: dir, path }
}

/// Review of Task 20 (re-check): audio left out for its container says so
/// (`UNSUPPORTED_CONTAINER`), not as a codec this build lacks.
#[test]
fn webm_audio_is_skipped_for_its_container() {
    if !gpu_or_skip("webm_audio_is_skipped_for_its_container") {
        return;
    }
    let c = webm_av_clip(&spec(6, Size::new(320, 180)));
    assert!(
        demux(&c.path).info().audio.is_some(),
        "the fixture has audio"
    );
    let job = job_for("fp", &c.path);
    let p = params_with("fp", ProxyTier::Scrub, ProxySourceAudio::Unavailable);
    let r = outcome(&run(&job, &p), "fp").unwrap();
    assert_eq!(r.audio, None);
    assert_eq!(
        r.audio_skipped,
        Some(ProxyAudioSkipped {
            code: "UNSUPPORTED_CONTAINER".into()
        })
    );
    assert_eq!(ids(&entry(&job, &r.key).join(PROXY_FILE)), ids(&c.path));
}
