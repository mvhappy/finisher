//! The CPU reference render of a `media.look` job directory (the e2e `look` scenario's oracle,
//! spec §13): `cargo run -p fin-workers --example reference_render -- <jobdir> [--png <file>]…`.
//!
//! Reads `in/params.json`, `in/snapshot.json` and `in/manifest.json` as `fin-workers media
//! --replay` does, evaluates each time's plan (no measurer: captions are not drawn in 6A),
//! decodes the same source pictures (`FrameReader`, `frame_at`) and draws them with the CPU
//! reference. Prints one JSON line: `{"frames": [{"at", "blake3", "t1"?}]}`, `blake3` being the
//! T0 hash of the reference's RGBA8 (identical on every machine given the same decoded
//! pixels). With `--png` (one per time, in order) each frame is also compared with that PNG
//! under tolerance T1: `t1 = {psnr, maxAbs, over2, pass}` (`psnr` is `null` when infinite).
//! The reference never reads fonts, the GPU or the network.
// A developer tool: it reads the job directory and the originals by path.
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use fin_codecs::{FrameReader, ProbeResult};
use fin_compositor::{LayerIssue, SourceFrame, SourceProvider, compare, reference};
use fin_container::Limits;
use fin_core::eval::{Layer, eval_frame};
use fin_media::jobs::{JobSnapshot, LookParams, RenderManifest};
use fin_media::{Size, frame_at};
use serde_json::{Value, json};

/// Decoded pictures for the reference, read with the worker's `FrameReader` and `frame_at`.
struct Decoded<'a> {
    manifest: &'a RenderManifest,
    readers: HashMap<String, FrameReader<File>>,
}

fn issue(code: &str, details: Value) -> LayerIssue {
    LayerIssue {
        code: code.into(),
        details,
    }
}

impl SourceProvider for Decoded<'_> {
    fn frame(&mut self, l: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        let s = l
            .source
            .as_ref()
            .ok_or_else(|| issue("LAYER_UNAVAILABLE", json!({ "reason": "noSource" })))?;
        if !self.readers.contains_key(&s.asset) {
            let original = self
                .manifest
                .asset(&s.asset)
                .and_then(|a| a.original.as_ref())
                .ok_or_else(|| issue("ORIGINALS_MISSING", json!({ "asset": s.asset })))?;
            let file = File::open(&original.path).map_err(|e| {
                issue(
                    "ORIGINALS_MISSING",
                    json!({ "asset": s.asset, "io": e.to_string() }),
                )
            })?;
            let reader = FrameReader::open(file, &Limits::default(), &ProbeResult::software_only())
                .map_err(|e| issue(&e.code, json!({ "asset": s.asset })))?;
            self.readers.insert(s.asset.clone(), reader);
        }
        let reader = self.readers.get_mut(&s.asset).expect("just inserted");
        let video = reader
            .info()
            .video
            .clone()
            .ok_or_else(|| issue("ORIGINALS_MISSING", json!({ "reason": "noVideo" })))?;
        let index = frame_at(reader.table(), s.time).index;
        let f = reader
            .frame(index)
            .map_err(|e| issue(&e.code, json!({ "asset": s.asset })))?;
        Ok(SourceFrame {
            format: f.format,
            size: f.size,
            planes: [&f.planes[0].data, &f.planes[1].data, &f.planes[2].data],
            strides: [f.planes[0].stride, f.planes[1].stride, f.planes[2].stride],
            // The container's tags (the fixtures' bitstream agrees with them), then spec §10.1's rule.
            colour: video.colour.resolve_defaults(f.size.height),
            hdr: video.hdr,
            rotation: video.rotation,
            pixel_aspect: video.pixel_aspect,
        })
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("reference_render: {msg}");
    eprintln!("usage: reference_render <jobdir> [--png <file>]…");
    std::process::exit(2)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes = std::fs::read(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())))
}

/// The RGBA8 pixels of a PNG and its size.
fn read_png(path: &Path) -> (Size, Vec<u8>) {
    let file = File::open(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buf)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        fail(&format!("{}: not an 8-bit RGBA PNG", path.display()));
    }
    buf.truncate(info.buffer_size());
    (Size::new(info.width, info.height), buf)
}

fn main() {
    let mut dir = None;
    let mut pngs: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--png" => pngs.push(
                args.next()
                    .unwrap_or_else(|| fail("--png needs a file"))
                    .into(),
            ),
            other if dir.is_none() && !other.starts_with("--") => dir = Some(PathBuf::from(other)),
            other => fail(&format!("unknown argument {other:?}")),
        }
    }
    let dir = dir.unwrap_or_else(|| fail("a job directory is required"));
    let params: LookParams = read_json(&dir.join("in/params.json"));
    let snapshot: JobSnapshot = read_json(&dir.join("in/snapshot.json"));
    let manifest: RenderManifest = read_json(&dir.join("in/manifest.json"));
    if !pngs.is_empty() && pngs.len() != params.at.len() {
        fail(&format!(
            "{} PNGs for {} times",
            pngs.len(),
            params.at.len()
        ));
    }
    let mut src = Decoded {
        manifest: &manifest,
        readers: HashMap::new(),
    };
    let mut frames = Vec::new();
    for (i, &t) in params.at.iter().enumerate() {
        let plan = eval_frame(&snapshot.project, &params.timeline, t, None)
            .unwrap_or_else(|e| fail(&format!("eval at {t}: {e}")));
        let size = params
            .effective_size(Size::new(plan.resolution.width, plan.resolution.height))
            .unwrap_or_else(|e| fail(&format!("size: {e}")));
        let rendered = reference::render(&plan, &mut src, size);
        let mut frame = json!({
            "at": t,
            "blake3": blake3::hash(&rendered.rgba8).to_hex().to_string(),
        });
        if let Some(path) = pngs.get(i) {
            let (got_size, rgba) = read_png(path);
            let pass = got_size == size;
            let t1 = compare::t1(&rgba, &rendered.rgba8);
            frame["t1"] = json!({
                "psnr": t1.psnr.is_finite().then_some(t1.psnr),
                "maxAbs": t1.max_abs,
                "over2": t1.over_2,
                "sizeMatches": pass,
                "pass": pass && compare::passes(&t1),
            });
        }
        frames.push(frame);
    }
    println!("{}", json!({ "frames": frames }));
}
