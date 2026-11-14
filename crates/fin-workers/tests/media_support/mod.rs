//! Fixtures for the `media` family tests: a project built with `fin-core` ops over an AV1
//! pattern clip (`fin-codecs`' `pattern_clip`) and an H.264 file no decoder reads, its job
//! directory (`in/snapshot.json`, `in/manifest.json`, `out/`), plan samples, and the CPU
//! reference over the same decoded frames.
// Tests write fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use fin_codecs::FrameReader;
use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_compositor::{AdapterChoice, Gpu, LayerIssue, SourceFrame, SourceProvider};
use fin_container::{CodecConfig, Limits, Mp4Options, Mp4Writer, VideoTrackConfig};
use fin_core::eval::{FramePlan, Layer, eval_frame};
use fin_core::{
    AssetAdd, InsertMode, ItemBody, ItemInsert, MemIds, Op, Project, Rational, Resolution,
    SCHEMA_VERSION, Tx, VideoInfo, apply_tx, new_project,
};
use fin_media::jobs::{
    JobSnapshot, LookParams, MANIFEST_VERSION, ManifestAsset, ManifestFonts, OriginalFile,
    PlanSample, RenderManifest,
};
use fin_media::{ColourTags, Rotation, Size, frame_at};
use serde_json::Value;

use crate::support::{Job, expect_of};

pub const CLIP_FRAMES: u32 = 64;
pub const CLIP_SIZE: Size = Size::new(320, 180);
/// Every look renders at this size (the 1920×1080 timeline at a third).
pub const OUT: Size = Size::new(640, 360);

/// `"n"` or `"n/d"`, reduced.
pub fn r(s: &str) -> Rational {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    Rational::new(n.parse().unwrap(), d.parse().unwrap()).unwrap()
}

/// Whether the GPU tests run: an adapter `FIN_GPU` names exists, else a notice and `false`
/// (`FIN_REQUIRE_GPU=1` fails instead).
pub fn gpu_or_skip(test: &str) -> bool {
    match AdapterChoice::from_env().and_then(Gpu::new) {
        Ok(_) => true,
        Err(e) if std::env::var("FIN_REQUIRE_GPU").as_deref() == Ok("1") => {
            panic!("{test}: FIN_REQUIRE_GPU=1 but {e}")
        }
        Err(e) => {
            eprintln!("{test}: skipped, no GPU adapter ({e})");
            false
        }
    }
}

/// The environment a host would give the worker: `FIN_GPU` when set, and the Vulkan loader's
/// ICD variables when the test run names a driver (a software one on a host without a GPU).
pub fn worker_env() -> Vec<(&'static str, String)> {
    [
        "FIN_GPU",
        "VK_ICD_FILENAMES",
        "VK_DRIVER_FILES",
        "VK_ADD_DRIVER_FILES",
    ]
    .into_iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| (k, v)))
    .collect()
}

/// An MP4 with one H.264 sample entry and one sample (no decoder in 6A reads it).
fn h264_file() -> Vec<u8> {
    let mut w = Mp4Writer::new(Cursor::new(Vec::new()), Mp4Options::default()).unwrap();
    let track = w
        .add_video(VideoTrackConfig {
            // avcC: version 1, High profile, level 4.0, 4-byte lengths, no parameter sets.
            config: CodecConfig::Avc {
                avcc: vec![1, 100, 0, 40, 0xff, 0xe0, 0],
            },
            coded: Size::new(320, 180),
            timescale: 30_000,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
            colour: ColourTags::BT709,
            hdr: None,
        })
        .unwrap();
    w.write(track, &[0, 0, 0, 1, 0x65], 0, 0, 30_000, true)
        .unwrap();
    w.finish().unwrap().into_inner()
}

/// The media files and the project over them.
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub av1: PathBuf,
    pub h264: PathBuf,
    pub project: Project,
    /// Asset ids as the ops allocated them.
    pub av1_id: String,
    pub h264_id: String,
}

fn tx(ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: "tx-fixture".into(),
        author: "test".into(),
        branch: "main".into(),
        base_rev: 0,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    }
}

impl Fixture {
    /// `t1`: the AV1 clip (64 frames at 30 fps, 320×180) for 0–2 s from its start, then the
    /// H.264 file for 2–3 s.
    pub fn new() -> Fixture {
        Fixture::with_clip(ClipSpec::default())
    }

    /// The fixture with the AV1 clip `spec` makes (`frames`, `size` and `keyint` are the
    /// fixture's): a VFR or edit-listed clip, say.
    pub fn with_clip(spec: ClipSpec) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let av1 = dir.path().join("pattern.mp4");
        let spec = ClipSpec {
            frames: CLIP_FRAMES,
            size: CLIP_SIZE,
            keyint: 16,
            ..spec
        };
        std::fs::write(&av1, pattern_clip(&spec).unwrap()).unwrap();
        let h264 = dir.path().join("other.mp4");
        std::fs::write(&h264, h264_file()).unwrap();
        let video = |w, h| VideoInfo {
            width: w,
            height: h,
            rate: Rational::from_int(30),
            display: None,
            rotation: None,
            pixel_aspect: None,
        };
        let clip = |id: &str, at: i64, asset: &str, secs: i64| -> Op {
            ItemInsert {
                id: id.into(),
                track: "t1".into(),
                at: Rational::from_int(at),
                duration: Rational::from_int(secs),
                mode: InsertMode::Ripple,
                body: ItemBody::Clip {
                    asset: asset.into(),
                    source_in: Rational::zero(),
                    speed: Rational::from_int(1),
                },
            }
            .into()
        };
        let ops = vec![
            AssetAdd::media(
                "$v",
                av1.to_str().unwrap(),
                Rational::new(CLIP_FRAMES as i64, 30).unwrap(),
                Some(video(320, 180)),
                None,
                "fp-av1",
            )
            .into(),
            AssetAdd::media(
                "$h",
                h264.to_str().unwrap(),
                Rational::from_int(1),
                Some(video(320, 180)),
                None,
                "fp-h264",
            )
            .into(),
            clip("$c1", 0, "$v", 2),
            clip("$c2", 2, "$h", 1),
        ];
        let mut p = new_project("p1".into(), "Look".into(), Rational::from_int(30)).unwrap();
        // Landscape, as the clips (a new project's timeline is portrait).
        p.timelines[0].resolution = Resolution {
            width: 1920,
            height: 1080,
        };
        let project = apply_tx(&p, &tx(ops), &mut MemIds::after(&p))
            .unwrap()
            .project;
        let id_of = |path: &Path| {
            project
                .assets
                .values()
                .find(|a| a.path == path.to_str().unwrap())
                .unwrap()
                .id
                .clone()
        };
        Fixture {
            av1_id: id_of(&av1),
            h264_id: id_of(&h264),
            av1,
            h264,
            project,
            dir,
        }
    }

    /// The plan the host would sample at `t`.
    pub fn plan(&self, t: Rational) -> FramePlan {
        eval_frame(&self.project, "tl1", t, None).unwrap()
    }

    pub fn manifest(&self) -> RenderManifest {
        let original = |p: &Path| {
            let e = serde_json::to_value(expect_of(p)).unwrap();
            OriginalFile {
                path: p.to_str().unwrap().into(),
                size: e["size"].as_u64().unwrap(),
                mtime_ns: e["mtimeNs"].as_str().unwrap().into(),
                file_id: e.get("fileId").and_then(Value::as_str).map(str::to_owned),
            }
        };
        let asset = |id: &str, fp: &str, p: &Path| ManifestAsset {
            asset: id.into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: fp.into(),
            original: Some(original(p)),
            proxies: Vec::new(),
        };
        RenderManifest {
            version: MANIFEST_VERSION,
            assets: vec![
                asset(&self.av1_id, "fp-av1", &self.av1),
                asset(&self.h264_id, "fp-h264", &self.h264),
            ],
            fonts: ManifestFonts::default(),
        }
    }

    /// `media.look` params for `at` with correct samples.
    pub fn params(&self, at: &[Rational]) -> LookParams {
        LookParams {
            timeline: "tl1".into(),
            at: at.to_vec(),
            size: Some(OUT),
            plan_samples: at
                .iter()
                .map(|&t| PlanSample {
                    at: t,
                    blake3: plan_hash(&self.plan(t)),
                })
                .collect(),
            out_name: "look".into(),
            project_id: None,
            branch: None,
            rev: None,
            out_dir: None,
            originals: false,
        }
    }

    /// A job directory with this fixture's inputs (and `manifest` when given).
    pub fn job(&self, manifest: Option<&RenderManifest>) -> Job {
        let job = Job::new();
        let snapshot = JobSnapshot {
            branch: "main".into(),
            rev: 1,
            project: self.project.clone(),
        };
        std::fs::write(
            job.path("in/snapshot.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let m = manifest.cloned().unwrap_or_else(|| self.manifest());
        std::fs::write(
            job.path("in/manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        job
    }

    /// The presented frame index of the AV1 clip `frame_at` picks at timeline time `t`.
    pub fn expected_frame(&self, t: Rational) -> u32 {
        let plan = self.plan(t);
        let s = plan.layers[0].source.as_ref().unwrap();
        let reader = FrameReader::open(
            File::open(&self.av1).unwrap(),
            &Limits::default(),
            &fin_codecs::ProbeResult::software_only(),
        )
        .unwrap();
        frame_at(reader.table(), s.time).index as u32
    }

    /// The CPU reference's frame at `t` over the same decoded pictures.
    pub fn reference(&self, t: Rational) -> Vec<u8> {
        let mut src = Decoded {
            paths: HashMap::from([
                (self.av1_id.clone(), self.av1.clone()),
                (self.h264_id.clone(), self.h264.clone()),
            ]),
            readers: HashMap::new(),
        };
        fin_compositor::reference::render(&self.plan(t), &mut src, OUT).rgba8
    }
}

pub fn plan_hash(plan: &FramePlan) -> String {
    blake3::hash(serde_json::to_string(plan).unwrap().as_bytes())
        .to_hex()
        .to_string()
}

/// Decoded pictures for the reference, read with the same `FrameReader` and `frame_at`.
struct Decoded {
    paths: HashMap<String, PathBuf>,
    readers: HashMap<String, FrameReader<File>>,
}

impl SourceProvider for Decoded {
    fn frame(&mut self, l: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        let s = l.source.as_ref().unwrap();
        let issue = |code: &str| LayerIssue {
            code: code.into(),
            details: serde_json::json!({}),
        };
        if !self.readers.contains_key(&s.asset) {
            let path = self
                .paths
                .get(&s.asset)
                .ok_or_else(|| issue("ORIGINALS_MISSING"))?;
            let reader = FrameReader::open(
                File::open(path).unwrap(),
                &Limits::default(),
                &fin_codecs::ProbeResult::software_only(),
            )
            .map_err(|e| issue(&e.code))?;
            self.readers.insert(s.asset.clone(), reader);
        }
        let reader = self.readers.get_mut(&s.asset).unwrap();
        let video = reader.info().video.clone().unwrap();
        let index = frame_at(reader.table(), s.time).index;
        let f = reader.frame(index).unwrap();
        Ok(SourceFrame {
            format: f.format,
            size: f.size,
            planes: [&f.planes[0].data, &f.planes[1].data, &f.planes[2].data],
            strides: [f.planes[0].stride, f.planes[1].stride, f.planes[2].stride],
            colour: video.colour.resolve_defaults(f.size.height),
            hdr: video.hdr,
            rotation: video.rotation,
            pixel_aspect: video.pixel_aspect,
        })
    }
}

/// The RGBA pixels of a PNG.
pub fn read_png(path: &Path) -> (Size, Vec<u8>) {
    let d = png::Decoder::new(std::io::BufReader::new(File::open(path).unwrap()));
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    (Size::new(info.width, info.height), buf)
}

/// Writes `params` as `in/params.json` (what `--replay` reads by default).
pub fn write_params(job: &Job, params: &LookParams) {
    std::fs::write(
        job.path("in/params.json"),
        serde_json::to_vec(params).unwrap(),
    )
    .unwrap();
}

/// A proxy cache entry of the fixture's AV1 clip, in a cache directory of its own
/// (`<root>/<key[0..2]>/<key>/{proxy.mp4,table.bin,meta.json}`).
pub struct ProxyEntry {
    pub _root: tempfile::TempDir,
    pub dir: PathBuf,
    pub key: String,
}

impl ProxyEntry {
    /// The manifest's view of the entry.
    pub fn listed(&self) -> fin_media::jobs::ManifestProxy {
        fin_media::jobs::ManifestProxy {
            tier: fin_media::jobs::ProxyTier::Preview,
            key: self.key.clone(),
            dir: self.dir.to_str().unwrap().into(),
            codec: fin_media::jobs::ProxyCodec::Av1,
        }
    }
}

/// Runs `media.proxy` (tier `1080`, AV1) on the fixture's AV1 clip and files its output the way
/// the cache does.
pub fn make_proxy(fx: &Fixture) -> ProxyEntry {
    use fin_media::backend::VendorClass;
    use fin_media::jobs::{ProxyCodec, ProxyParams, ProxyTier, proxy_key};
    let tier = ProxyTier::Preview;
    let key = proxy_key(
        "fp-av1",
        tier,
        ProxyCodec::Av1,
        VendorClass::Software,
        fin_media::jobs::ProxySourceAudio::None,
    )
    .to_string();
    let params = ProxyParams {
        fingerprint: "fp-av1".into(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        audio: fin_media::jobs::ProxySourceAudio::None,
        key: key.clone(),
        asset: Some(fx.av1_id.clone()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    };
    let job = Job::new();
    let manifest = RenderManifest {
        assets: vec![fx.manifest().asset(&fx.av1_id).unwrap().clone()],
        ..fx.manifest()
    };
    std::fs::write(
        job.path("in/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let mut w = crate::support::spawn_with(&["media"], &worker_env());
    w.hello();
    let start = job.start(
        "media.proxy",
        serde_json::to_value(&params).unwrap(),
        &["in/manifest.json", "out/"],
    );
    w.send(&fin_jobs::HostMsg::Start(start));
    let run = w.drive(crate::support::no_calls);
    assert!(
        matches!(run.end, crate::support::End::Result(_)),
        "{:?}",
        run.end
    );
    assert!(
        run.events
            .iter()
            .any(|e| matches!(e, crate::support::Ev::Item(_, Some(_)))),
        "a proxy: {:?}",
        run.events
    );
    let root = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(root.path())
        .unwrap()
        .join(&key[..2])
        .join(&key);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["proxy.mp4", "table.bin", "meta.json"] {
        std::fs::copy(job.out().join(&key).join(f), dir.join(f)).unwrap();
    }
    ProxyEntry {
        _root: root,
        dir,
        key,
    }
}

impl Fixture {
    /// The manifest with `proxy` listed for the AV1 asset.
    pub fn manifest_with(&self, proxy: &ProxyEntry) -> RenderManifest {
        let mut m = self.manifest();
        for a in &mut m.assets {
            if a.asset == self.av1_id {
                a.proxies = vec![proxy.listed()];
            }
        }
        m
    }
}

/// PSNR in dB over the colour channels of two RGBA8 images (infinite when equal).
pub fn psnr_rgb(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let (mut sq, mut n) = (0f64, 0f64);
    // Every fourth byte is alpha.
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if i % 4 != 3 {
            sq += (f64::from(*x) - f64::from(*y)).powi(2);
            n += 1.0;
        }
    }
    if sq == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / (sq / n)).log10()
    }
}

/// Mean SSIM over the luma (BT.709 weights) of two RGBA8 images of `width`, in 8×8 windows
/// stepped by 4 (Wang et al. 2004, uniform window, `K1` 0.01, `K2` 0.03, `L` 255).
pub fn ssim_luma(a: &[u8], b: &[u8], width: u32) -> f64 {
    assert_eq!(a.len(), b.len());
    let luma = |p: &[u8]| -> Vec<f64> {
        p.as_chunks::<4>()
            .0
            .iter()
            .map(|px| {
                0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
            })
            .collect()
    };
    let (x, y) = (luma(a), luma(b));
    let w = width as usize;
    let h = x.len() / w;
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    const WIN: usize = 8;
    let (mut total, mut windows) = (0f64, 0u32);
    for top in (0..=h - WIN).step_by(4) {
        for left in (0..=w - WIN).step_by(4) {
            let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64);
            for r in top..top + WIN {
                for c in left..left + WIN {
                    let (p, q) = (x[r * w + c], y[r * w + c]);
                    sx += p;
                    sy += q;
                    sxx += p * p;
                    syy += q * q;
                    sxy += p * q;
                }
            }
            let n = (WIN * WIN) as f64;
            let (mx, my) = (sx / n, sy / n);
            let vx = sxx / n - mx * mx;
            let vy = syy / n - my * my;
            let cov = sxy / n - mx * my;
            total += ((2.0 * mx * my + c1) * (2.0 * cov + c2))
                / ((mx * mx + my * my + c1) * (vx + vy + c2));
            windows += 1;
        }
    }
    total / f64::from(windows)
}
