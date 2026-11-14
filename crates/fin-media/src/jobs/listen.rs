//! `media.listen` v1 (plan 6C Task 21, Decision 21; sub-project 6 spec §8.7): what `fin listen`
//! measures over a range of a timeline, per track and for the mix, from the audio plan the worker
//! evaluates out of the pinned snapshot.
//!
//! - **Plan samples** (as `media.look`'s, plan 6A Decision 4): the BLAKE3 of the compact JSON of
//!   the audio plan of each 10 s window of the range (`[from, from + 10)`, `[from + 10, from + 20)`,
//!   …, the last one ending at `to`: [`listen_windows`]). The host computes them from the snapshot
//!   it writes ([`listen_plan_samples`]); the worker evaluates the same windows and refuses any
//!   difference (`PLAN_MISMATCH`).
//! - **The result** ([`ListenResult`]): the mix (the master bus, never normalised: `fin listen`
//!   reports, it does not change), each track's own mix (the same mixer over that track's items
//!   only), and notes (`missingSource`, `STRETCH_EXTREME`, `skippedEffect`). Levels that are
//!   minus infinity (silence) are absent, never NaN. Times are rational seconds on the timeline.

use fin_core::eval::{AudioPlan, eval_audio};
use fin_core::{CoreError, Project, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PlanSample;
use super::invalid;

/// The plan is sampled every this many seconds from the range start.
pub const LISTEN_SAMPLE_SECS: i64 = 10;

/// `media.listen` v1: measure `[from, to)` of `timeline`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListenParams {
    pub timeline: String,
    pub from: Rational,
    pub to: Rational,
    /// One sample per window of [`listen_windows`]; a plan the worker evaluates differently is
    /// `PLAN_MISMATCH`. A submitter leaves them out: the kind's `prepare` computes them from the
    /// snapshot it writes and hands them to the worker as `start.job.extra.planSamples` (and in
    /// `in/params.json` for `--replay`).
    #[serde(default)]
    pub plan_samples: Vec<PlanSample>,
    /// The project measured. Only the host reads it (the kind's `dedupe` and `prepare`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The branch of the snapshot (an id, a name or `<name>#<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch's rev the submitter saw: part of the job's identity (`dedupe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<u64>,
}

impl ListenParams {
    /// The checks serde cannot make: a range that starts at 0 or later, ends after it starts and
    /// lasts at most [`super::MAX_PCM_SECONDS`]. Its end past the timeline's is the host's check
    /// (`prepareListenJob`, which has the snapshot).
    pub fn check(&self) -> Result<(), CoreError> {
        if self.from.is_negative() {
            return Err(invalid(
                "from",
                format!("the range starts at 0 or later, not {}", self.from),
            ));
        }
        let longest = Rational::from_int(super::MAX_PCM_SECONDS as i64);
        if self
            .to
            .checked_sub(self.from)
            .is_ok_and(|len| len > longest)
        {
            return Err(invalid(
                "to",
                format!(
                    "the range is at most {} s long ({} to {})",
                    super::MAX_PCM_SECONDS,
                    self.from,
                    self.to
                ),
            ));
        }
        if self.to <= self.from {
            return Err(invalid(
                "to",
                format!(
                    "the range ends after it starts ({} to {})",
                    self.from, self.to
                ),
            ));
        }
        Ok(())
    }

    /// The sample for the window starting at `t` (the first, should several be given).
    pub fn sample(&self, t: Rational) -> Option<&PlanSample> {
        self.plan_samples.iter().find(|s| s.at == t)
    }
}

/// The windows the plan is sampled over: `[from + 10k, min(from + 10(k + 1), to))` for every `k`
/// whose window starts before `to`. `TIME_OVERFLOW` when a bound does not fit a rational.
pub fn listen_windows(
    from: Rational,
    to: Rational,
) -> Result<Vec<(Rational, Rational)>, CoreError> {
    let step = Rational::from_int(LISTEN_SAMPLE_SECS);
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let next = at.checked_add(step)?;
        out.push((at, next.min(to)));
        at = next;
    }
    Ok(out)
}

/// BLAKE3 of the audio plan's compact JSON, lowercase hex (as `media.look`'s frame plans).
pub fn audio_plan_hash(plan: &AudioPlan) -> String {
    let json = serde_json::to_string(plan).expect("a plan serialises");
    blake3::hash(json.as_bytes()).to_hex().to_string()
}

/// The plan samples of `[from, to)` of `timeline` in `project`: one per [`listen_windows`]
/// window, at its start. `eval_audio`'s errors (`NOT_FOUND`, `INVALID_RANGE`, …).
pub fn listen_plan_samples(
    project: &Project,
    timeline: &str,
    from: Rational,
    to: Rational,
) -> Result<Vec<PlanSample>, CoreError> {
    listen_windows(from, to)?
        .into_iter()
        .map(|(a, b)| {
            Ok(PlanSample {
                at: a,
                blake3: audio_plan_hash(&eval_audio(project, timeline, a, b)?),
            })
        })
        .collect()
}

/// Samples above full scale.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenClipped {
    /// Samples (each channel's) whose magnitude is above 1.0.
    pub samples: u64,
    /// The first frame holding one, on the timeline (exact: its sample index ÷ 48,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub first_at: Option<Rational>,
}

/// A span of the timeline, `[from, to)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenSpan {
    pub from: Rational,
    pub to: Rational,
}

/// One of the loudest moments: the 3 s window starting at `at` (clipped to the range start)
/// read `shortTerm` LUFS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenMoment {
    pub at: Rational,
    pub short_term: f64,
}

/// What one mix (the master bus, or one track's items alone) measured (Decision 21).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenStats {
    /// Integrated loudness, LUFS; absent when nothing is above the −70 LUFS gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "f64")]
    pub integrated: Option<f64>,
    /// Loudness range, LU.
    pub range: f64,
    /// dBTP; absent for silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "f64")]
    pub true_peak: Option<f64>,
    /// dBFS; absent for silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "f64")]
    pub sample_peak: Option<f64>,
    pub clipped: ListenClipped,
    /// Runs of 10 ms blocks whose louder channel's RMS is below −60 dBFS, lasting 0.5 s or more
    /// (or of any length when they touch an edge of the range, clipped there), at 10 ms resolution
    /// from the range start.
    pub silences: Vec<ListenSpan>,
    /// Up to 5 short-term (3 s) loudness maxima, loudest first, at least 3 s apart.
    pub loudest: Vec<ListenMoment>,
}

/// One track's own statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenTrack {
    /// The track of the measured timeline (a nested timeline's items count for the track their
    /// compound clip is on).
    pub track: String,
    pub stats: ListenStats,
}

/// Something the listen could not hear as planned; never a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "code")]
pub enum ListenNote {
    /// An asset with neither a `pcm48` entry nor a readable original here: its items are
    /// silent. `reason` is the opener's code (`ORIGINALS_MISSING`, `CODEC_UNAVAILABLE`, …).
    #[serde(rename = "missingSource")]
    MissingSource { asset: String, reason: String },
    /// An item played at a speed WSOLA cannot do well (below 1/4 or above 4: Decision 6).
    #[serde(rename = "STRETCH_EXTREME")]
    StretchExtreme { item: String, speed: Rational },
    /// An effect of a kind this engine does not know: its item plays without it.
    #[serde(rename = "skippedEffect")]
    SkippedEffect {
        item: String,
        effect: String,
        kind: String,
    },
}

/// Where the sources were read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenJobStats {
    /// Assets read from their `pcm48` cache entry (the manifest's `pcm`).
    pub sources_from_cache: u32,
    /// Assets decoded from their original (no entry, or one that did not verify).
    pub sources_decoded: u32,
}

/// What `media.listen` v1 answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenResult {
    pub timeline: String,
    /// The branch rev of the snapshot measured.
    pub rev: u64,
    pub from: Rational,
    pub to: Rational,
    pub mix: ListenStats,
    /// The tracks with items in the range, in the timeline's order.
    pub tracks: Vec<ListenTrack>,
    pub notes: Vec<ListenNote>,
    pub stats: ListenJobStats,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(n: i64, d: i64) -> Rational {
        Rational::new(n, d).unwrap()
    }

    fn params(from: Rational, to: Rational) -> ListenParams {
        ListenParams {
            timeline: "tl1".into(),
            from,
            to,
            plan_samples: vec![],
            project_id: None,
            branch: None,
            rev: None,
        }
    }

    #[test]
    fn windows_are_ten_seconds_from_the_start_and_end_at_the_range_end() {
        assert_eq!(
            listen_windows(r(1, 2), r(25, 1)).unwrap(),
            [
                (r(1, 2), r(21, 2)),
                (r(21, 2), r(41, 2)),
                (r(41, 2), r(25, 1))
            ]
        );
        assert_eq!(
            listen_windows(Rational::zero(), r(10, 1)).unwrap(),
            [(Rational::zero(), r(10, 1))]
        );
        assert!(listen_windows(r(3, 1), r(3, 1)).unwrap().is_empty());
    }

    #[test]
    fn ranges_are_checked() {
        assert!(params(Rational::zero(), r(1, 1)).check().is_ok());
        let e = params(r(1, 1), r(1, 1)).check().unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["field"]),
            ("JOB_PARAMS_INVALID", &json!("to"))
        );
        let e = params(r(-1, 1), r(1, 1)).check().unwrap_err();
        assert_eq!(e.details["field"], "from");
        // At most MAX_PCM_SECONDS long (6C Task 21 review S1).
        let max = super::super::MAX_PCM_SECONDS as i64;
        assert!(params(r(5, 1), r(5 + max, 1)).check().is_ok());
        let e = params(r(5, 1), r(6 + max, 1)).check().unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["field"]),
            ("JOB_PARAMS_INVALID", &json!("to"))
        );
        let mut v = serde_json::to_value(params(Rational::zero(), r(1, 1))).unwrap();
        v["surprise"] = json!(1);
        assert!(serde_json::from_value::<ListenParams>(v).is_err());
    }

    #[test]
    fn notes_carry_their_code() {
        let n = ListenNote::StretchExtreme {
            item: "i1".into(),
            speed: r(8, 1),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({ "code": "STRETCH_EXTREME", "item": "i1", "speed": "8" })
        );
        let m = ListenNote::MissingSource {
            asset: "a1".into(),
            reason: "ORIGINALS_MISSING".into(),
        };
        assert_eq!(serde_json::to_value(&m).unwrap()["code"], "missingSource");
    }
}
