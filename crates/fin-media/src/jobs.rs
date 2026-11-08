//! Media job params, inputs and results (spec §3.3, §4.2; plan 6A Task 11). Generated into
//! `@finisher/core-types`; the `media` worker family reads them, `fin-node` writes the inputs.
//!
//! - `in/snapshot.json` is a [`JobSnapshot`] (plan 6A Decision 2): the project at
//!   `(branch, rev)`, as `fin-node`'s `snapshot` returns it, with the branch beside it.
//! - `in/manifest.json` is a [`RenderManifest`] (spec §4.2): what each asset the snapshot
//!   references is on this machine, and the font bytes captions may use.
//! - `media.look` v1 takes [`LookParams`] and answers [`LookResult`]; each frame is also an
//!   `item` (`"0"`, `"1"`, … in `at` order) whose value is its [`LookFrame`].

use fin_core::eval::ItemKind;
use fin_core::events::payloads::JobMedia;
use fin_core::{AssetKind, CoreError, Project, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::format::VideoCodec;
use crate::info::Size;

/// The most times one `media.look` renders.
pub const LOOK_MAX_TIMES: usize = 64;

/// The longest side of a `media.look` frame.
pub const LOOK_MAX_SIDE: u32 = 8192;

/// The longest `outName`.
pub const OUT_NAME_MAX: usize = 64;

/// The BLAKE3 of a frame plan at one time (plan 6A Decision 4): lowercase hex of the plan's
/// compact JSON, as `fin-core`'s eval corpus hashes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanSample {
    pub at: Rational,
    pub blake3: String,
}

/// `media.look` v1: frames of `timeline` at each time in `at`, as PNGs in `out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookParams {
    pub timeline: String,
    /// Timeline times in seconds, 1 to [`LOOK_MAX_TIMES`] of them.
    pub at: Vec<Rational>,
    /// The frame size; the timeline's resolution when absent. Each side 1 to
    /// [`LOOK_MAX_SIDE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Size")]
    pub size: Option<Size>,
    /// One sample per time in `at` (spec §3.3: every time for `media.look`); a plan the worker
    /// evaluates differently is `PLAN_MISMATCH`. A submitter leaves them out: the kind's
    /// `prepare` computes them from the snapshot it writes and hands them to the worker as
    /// `start.job.extra.planSamples` (and in `in/params.json`, with the rest of these params,
    /// for `--replay`).
    #[serde(default)]
    pub plan_samples: Vec<PlanSample>,
    /// The stem of the files written: `out/<outName>-<i>.png`. `[A-Za-z0-9_-]`, 1 to
    /// [`OUT_NAME_MAX`] characters.
    pub out_name: String,
    /// The project the look is of. Only the host reads it (the kind's `dedupe` and `prepare`);
    /// the worker works from the job directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch of the snapshot (an id, a name or `<name>#<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch's rev the submitter saw: part of the job's identity (`dedupe`). `prepare`
    /// snapshots the head it finds, and `in/snapshot.json` says which rev that was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
    /// Where the kind's `onSucceeded` moves the frames: absolute, or relative to the project
    /// folder; `workspace/shared/look/` when absent. Never inside `.finisher/` or `assets/`
    /// (`EXPORT_DESTINATION`). The worker ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<String>,
}

/// Each side of a frame 1 to [`LOOK_MAX_SIDE`].
fn check_size(s: Size) -> Result<(), CoreError> {
    let side = 1..=LOOK_MAX_SIDE;
    if side.contains(&s.width) && side.contains(&s.height) {
        return Ok(());
    }
    Err(invalid(
        "size",
        format!(
            "each side 1 to {LOOK_MAX_SIDE}, not {}×{}",
            s.width, s.height
        ),
    ))
}

fn invalid(field: &str, message: String) -> CoreError {
    CoreError::new("JOB_PARAMS_INVALID", format!("params.{field}: {message}"))
        .with_details(json!({ "field": field }))
}

impl LookParams {
    /// The checks serde cannot make: counts, sizes, the name, one sample per time.
    pub fn check(&self) -> Result<(), CoreError> {
        if self.at.is_empty() || self.at.len() > LOOK_MAX_TIMES {
            return Err(invalid(
                "at",
                format!("1 to {LOOK_MAX_TIMES} times, not {}", self.at.len()),
            ));
        }
        if let Some(s) = self.size {
            check_size(s)?;
        }
        let ok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if self.out_name.is_empty()
            || self.out_name.len() > OUT_NAME_MAX
            || !self.out_name.chars().all(ok)
        {
            return Err(invalid(
                "outName",
                format!("[A-Za-z0-9_-]{{1,{OUT_NAME_MAX}}}, not {:?}", self.out_name),
            ));
        }
        Ok(())
    }

    /// The size frames are drawn at: `size`, else the timeline's resolution `res`. The limit
    /// of [`LOOK_MAX_SIDE`] applies to the size actually drawn, so a timeline larger than that
    /// needs a `size` (plan 6A Task 11-12 review M3).
    pub fn effective_size(&self, res: Size) -> Result<Size, CoreError> {
        match self.size {
            Some(s) => Ok(s),
            None => check_size(res).map(|()| res).map_err(|e| {
                e.with_fix("name a smaller size: the timeline's resolution is over the limit")
            }),
        }
    }

    /// The sample for time `t` (the first, should several be given).
    pub fn sample(&self, t: Rational) -> Option<&PlanSample> {
        self.plan_samples.iter().find(|s| s.at == t)
    }

    /// The name of frame `i`'s file in `out/`.
    pub fn file_name(&self, i: usize) -> String {
        format!("{}-{i}.png", self.out_name)
    }
}

/// One rendered frame of a `media.look`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LookFrame {
    pub at: Rational,
    /// Relative to `out/` in a worker's result; the frame's final absolute path in the job's
    /// stored result, once the kind's `onSucceeded` has moved it.
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// The plan's BLAKE3 (lowercase hex).
    pub plan: String,
}

/// A layer, or an effect or mask of a drawn one, that a frame left out (spec §2.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LookSkipped {
    pub at: Rational,
    pub item: String,
    pub kind: ItemKind,
    /// `CODEC_UNAVAILABLE`, `ORIGINALS_MISSING`, `LAYER_UNAVAILABLE`, `MEDIA_CORRUPT`, …
    pub code: String,
    #[schemars(with = "Value")]
    pub details: Value,
}

/// The GPU adapter frames were composited on (numeric ids, spec §21 decision 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GpuAdapter {
    pub backend: String,
    pub device_type: String,
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    /// The driver's major version, when its version text has a number (the text itself is
    /// not kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub driver_major: Option<u32>,
    /// A CPU adapter (lavapipe, WARP).
    pub software: bool,
}

/// What a warm worker reused (spec §3.2 item 1: caches keyed by content).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LookStats {
    /// The GPU device and pipelines were made by an earlier job of this worker.
    pub device_reused: bool,
    /// Sources opened (container parsed, decoder made) by this job.
    pub sources_opened: u32,
    /// Sources an earlier job of this worker had open.
    pub sources_reused: u32,
    /// Jobs this worker ran, this one included.
    pub worker_jobs: u32,
    // The `media` object of the job's `job.done` (6A Task 14): present when diagnostics were
    // on at the worker. `finisherd` reads it when the job ends; it holds no path or name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "JobMedia")]
    pub media: Option<JobMedia>,
}

/// What `media.look` v1 answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LookResult {
    /// In `at` order.
    pub frames: Vec<LookFrame>,
    /// Every frame's, in `at` order, bottom layer first within a frame.
    pub skipped: Vec<LookSkipped>,
    pub adapter: GpuAdapter,
    pub stats: LookStats,
}

/// `media.capabilities` v1 takes no params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilitiesParams {}

/// How a codec direction is served on this machine (spec §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CapSupport {
    Hardware,
    Software,
    None,
}

/// One codec's support: the shape of `fin-codecs`' `CodecSupport`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecCaps {
    pub codec: VideoCodec,
    pub decode: CapSupport,
    pub encode: CapSupport,
    /// The backends that would be used, decode first.
    pub backends: Vec<String>,
}

/// What this machine can decode and encode: the shape of `fin-codecs`' `Capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodecCapabilities {
    pub video: Vec<CodecCaps>,
}

/// The worker binary's versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerInfo {
    /// The `fin-workers` crate's version.
    pub version: String,
    /// [`RENDER_VERSION`](crate::RENDER_VERSION): part of every cache key (spec §2.5).
    pub render_version: u32,
}

/// What `media.capabilities` v1 answers (plan 6A Task 13): codecs, the GPU a render would use
/// (`null` when no adapter, not even a software one, can be opened) and the worker's versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilities {
    pub codecs: CodecCapabilities,
    pub gpu: Option<GpuAdapter>,
    pub worker: WorkerInfo,
}

/// `in/snapshot.json` (plan 6A Decision 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub branch: String,
    pub rev: u64,
    pub project: Project,
}

/// The render manifest's version.
pub const MANIFEST_VERSION: u32 = 1;

/// `in/manifest.json` (spec §4.2): built by `fin-node` from the snapshot's asset table,
/// never by the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenderManifest {
    /// [`MANIFEST_VERSION`].
    pub version: u32,
    /// Every asset the snapshot references, once.
    pub assets: Vec<ManifestAsset>,
    #[serde(default)]
    pub fonts: ManifestFonts,
}

impl RenderManifest {
    pub fn asset(&self, id: &str) -> Option<&ManifestAsset> {
        self.assets.iter().find(|a| a.asset == id)
    }
}

/// One asset of the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestAsset {
    pub asset: String,
    pub kind: AssetKind,
    pub fingerprint: String,
    /// The file on this machine (plan 6A Decision 5: the model's stored path), `null` when it
    /// is missing.
    pub original: Option<OriginalFile>,
}

/// A file a worker opens, with what it looked like when the manifest was written.
///
/// `path` may be absolute anywhere on the machine (the model stores the path the user gave:
/// plan 6A Decision 5) and the directories above the file may be links: **by design** only the
/// file itself is opened no-follow and checked against these facts. The manifest is the host's,
/// written from the project's own model, never from anything a worker or a client sent in the
/// job; `--replay` of a directory that is not the caller's own is the exception, and confines
/// originals to the directory (`--originals-anywhere` lifts that). The worker
/// opens it through `fin-library`'s no-follow opener, which refuses it once its size, mtime or
/// file id changed (`ORIGINALS_MISSING {reason: changed}`); `mtimeNs` and `fileId` are
/// encoded as `fin-library`'s `Expect` encodes them (a decimal string; standard base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OriginalFile {
    /// Absolute.
    pub path: String,
    pub size: u64,
    pub mtime_ns: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub file_id: Option<String>,
}

/// Font bytes for captions (plan 6A Decision 3): the server's font cache, read-only, then the
/// project's font asset files by SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFonts {
    /// `<server-data>/fonts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cache: Option<String>,
    #[serde(default)]
    pub files: Vec<FontFileRef>,
}

/// A project font asset's file and the SHA-256 its asset pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontFileRef {
    pub sha256: String,
    pub original: OriginalFile,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> LookParams {
        LookParams {
            timeline: "tl1".into(),
            at: vec![Rational::from_int(1)],
            size: None,
            plan_samples: vec![],
            out_name: "look".into(),
            project_id: None,
            branch: None,
            rev: None,
            out_dir: None,
        }
    }

    #[test]
    fn checks_counts_sizes_and_names() {
        assert!(params().check().is_ok());
        let mut p = params();
        p.at.clear();
        assert_eq!(p.check().unwrap_err().code, "JOB_PARAMS_INVALID");
        p.at = vec![Rational::zero(); LOOK_MAX_TIMES + 1];
        assert!(p.check().is_err());
        let mut p = params();
        p.size = Some(Size::new(0, 10));
        assert!(p.check().is_err());
        p.size = Some(Size::new(LOOK_MAX_SIDE, 1));
        assert!(p.check().is_ok());
        for bad in ["", "../x", "a b", "a.png", &"x".repeat(OUT_NAME_MAX + 1)] {
            let mut p = params();
            p.out_name = bad.into();
            assert!(p.check().is_err(), "{bad:?}");
        }
        assert_eq!(params().file_name(3), "look-3.png");
    }

    #[test]
    fn the_effective_size_is_limited_whether_named_or_not() {
        let big = Size::new(LOOK_MAX_SIDE + 1, 1080);
        let mut p = params();
        assert_eq!(
            p.effective_size(Size::new(1920, 1080)).unwrap(),
            Size::new(1920, 1080)
        );
        let e = p.effective_size(big).unwrap_err();
        assert_eq!(e.code, "JOB_PARAMS_INVALID");
        assert_eq!(e.details["field"], "size");
        p.size = Some(Size::new(640, 360));
        assert_eq!(p.effective_size(big).unwrap(), Size::new(640, 360));
        assert!(
            params()
                .effective_size(Size::new(LOOK_MAX_SIDE, LOOK_MAX_SIDE))
                .is_ok()
        );
    }

    #[test]
    fn unknown_fields_are_refused() {
        let mut v = serde_json::to_value(params()).unwrap();
        v["surprise"] = json!(1);
        assert!(serde_json::from_value::<LookParams>(v).is_err());
    }
}
