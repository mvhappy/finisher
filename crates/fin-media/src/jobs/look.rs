//! `media.look` v1 (plan 6A Task 11): params, frames, the adapter and the reuse counts.

use fin_core::eval::ItemKind;
use fin_core::events::payloads::JobMedia;
use fin_core::{CoreError, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::invalid;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
