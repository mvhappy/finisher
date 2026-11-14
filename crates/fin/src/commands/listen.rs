//! `fin listen [--from a] [--to b] [--json] [--no-wait]` (plan 6C Task 21, spec §8.7): what a
//! range of a timeline sounds like, measured by a `media.listen` job on the server's media worker:
//! integrated loudness, loudness range, true and sample peak, clipping, silences and the loudest
//! moments, for the mix and per track. `fin` links no media crate: it resolves the times, sends
//! the request and prints the answer.
//!
//! - Times are typed as anywhere else (`12.4s`, `00:12.4`, `f300`, `62/5`) and echoed resolved;
//!   the range defaults to the whole timeline (`--timeline`, the global flag, picks it: default the
//!   first). A range past the timeline's end is `INVALID_RANGE`.
//! - Output: by default a summary, `{timeline, from, to, lines}` (one line for the mix, one per
//!   track, then the notes); `--json` answers the whole `ListenResult` (`mix`, `tracks`, `notes`,
//!   `stats`). `--no-wait` answers `{timeline, from, to, job}` at once: `fin job wait <job.id>`
//!   waits for it and `fin job show` reads its result.
//! - The mix is the master bus as the plan has it, never normalised: `fin listen` reports, it does
//!   not change. A source this machine cannot read is a note, not an error.

use std::time::Duration;

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::client::Client;
use crate::endpoint;
use crate::output::unavailable;

/// How long the server waits for the job (finisherd's `LISTEN_WAIT_MS`).
const LISTEN_WAIT: Duration = Duration::from_secs(10 * 60);
/// The server's answer after its wait: a margin for it to reach fin.
const WAIT_SLACK: Duration = Duration::from_secs(30);

/// Loudness, peaks, clipping and silences of a range, per track and for the mix.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListenArgs {
    /// The range's start (default 0).
    #[arg(long, value_name = "t", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The range's end (default the timeline's end).
    #[arg(long, value_name = "t", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Answer the whole result (mix, tracks, notes, stats) instead of the summary lines.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub json: bool,
    /// Answer `{job}` at once instead of waiting for the measurements.
    #[arg(long = "no-wait")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_wait: bool,
}

/// A level with one decimal, or `-inf` for an absent one (silence).
fn level(v: &Value, unit: &str) -> String {
    v.as_f64()
        .map_or_else(|| format!("-inf {unit}"), |x| format!("{x:.1} {unit}"))
}

/// One summary line of a mix's statistics.
fn line(name: &str, s: &Value) -> String {
    let mut out = format!(
        "{name}: {} integrated, {} range, {} true peak, {} sample peak",
        level(&s["integrated"], "LUFS"),
        level(&s["range"], "LU"),
        level(&s["truePeak"], "dBTP"),
        level(&s["samplePeak"], "dBFS"),
    );
    let clipped = s["clipped"]["samples"].as_u64().unwrap_or(0);
    if clipped > 0 {
        let at = s["clipped"]["firstAt"].as_str().unwrap_or("?");
        out.push_str(&format!("; {clipped} clipped samples from {at}s"));
    }
    let silences: Vec<String> = s["silences"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| {
            format!(
                "{}–{}s",
                x["from"].as_str().unwrap_or("?"),
                x["to"].as_str().unwrap_or("?")
            )
        })
        .collect();
    if !silences.is_empty() {
        out.push_str(&format!("; silent {}", silences.join(", ")));
    }
    let loudest: Vec<String> = s["loudest"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            format!(
                "{}s ({})",
                m["at"].as_str().unwrap_or("?"),
                level(&m["shortTerm"], "LUFS")
            )
        })
        .collect();
    if !loudest.is_empty() {
        out.push_str(&format!("; loudest {}", loudest.join(", ")));
    }
    out
}

/// A note as a line: its code and its fields.
fn note_line(n: &Value) -> String {
    let code = n["code"].as_str().unwrap_or("note");
    let fields: Vec<String> = n
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| k.as_str() != "code")
        .map(|(k, v)| {
            format!(
                "{k} {}",
                v.as_str().map_or_else(|| v.to_string(), str::to_owned)
            )
        })
        .collect();
    format!("note {code}: {}", fields.join(", "))
}

/// The summary lines of a `ListenResult`: the mix, each track, then the notes.
pub fn summary(result: &Value) -> Vec<String> {
    let mut lines = vec![line("mix", &result["mix"])];
    for t in result["tracks"].as_array().into_iter().flatten() {
        lines.push(line(
            &format!("track {}", t["track"].as_str().unwrap_or("?")),
            &t["stats"],
        ));
    }
    lines.extend(
        result["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(note_line),
    );
    lines
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &ListenArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": "media.listen" }))
                .with_fix("fin listen measures on the server: start finisherd and run it again"),
        );
    }
    let branch = target_branch(backend, globals)?;
    let snap = backend.snapshot(&branch)?;
    let tl = addr::timeline(&snap.project, globals.timeline.as_deref())?;
    let mut req = json!({ "branch": branch, "timeline": tl.id });
    if let Some(from) = &a.from {
        req["from"] = json!(addr::time_at(tl.rate, "from", from.trim())?);
    }
    if let Some(to) = &a.to {
        req["to"] = json!(addr::time_at(tl.rate, "to", to.trim())?);
    }
    let mut answer = if a.no_wait {
        req["wait"] = json!(false);
        backend.project_call("media.listen", req.clone())?
    } else {
        req["projectId"] = json!(backend.project_id()?);
        // A long call with its own timeout: outside the overall deadline, as `fin job wait`.
        let mut client =
            Client::connect(&endpoint::resolve()?, LISTEN_WAIT + WAIT_SLACK)?.without_deadline();
        client.call("media.listen", req.clone())?
    };
    if a.no_wait {
        let mut out = json!({ "timeline": tl.id, "job": answer["job"].take() });
        for k in ["from", "to"] {
            if let Some(v) = req.get(k) {
                out[k] = v.clone();
            }
        }
        return Ok(out);
    }
    if a.json {
        return Ok(answer);
    }
    Ok(json!({
        "timeline": answer["timeline"].take(),
        "from": answer["from"].take(),
        "to": answer["to"].take(),
        "lines": summary(&answer),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_has_a_line_for_the_mix_each_track_and_each_note() {
        let stats = json!({
            "integrated": -16.99, "range": 0.4, "truePeak": -1.24, "samplePeak": -1.3,
            "clipped": { "samples": 3, "firstAt": "3/2" },
            "silences": [{ "from": "5", "to": "6" }],
            "loudest": [{ "at": "0", "shortTerm": -16.5 }],
        });
        let silent =
            json!({ "range": 0, "clipped": { "samples": 0 }, "silences": [], "loudest": [] });
        let r = json!({
            "mix": stats,
            "tracks": [{ "track": "t2", "stats": silent }],
            "notes": [{ "code": "missingSource", "asset": "a2", "reason": "ORIGINALS_MISSING" }],
        });
        assert_eq!(
            summary(&r),
            [
                "mix: -17.0 LUFS integrated, 0.4 LU range, -1.2 dBTP true peak, -1.3 dBFS sample peak; 3 clipped samples from 3/2s; silent 5–6s; loudest 0s (-16.5 LUFS)",
                "track t2: -inf LUFS integrated, 0.0 LU range, -inf dBTP true peak, -inf dBFS sample peak",
                "note missingSource: asset a2, reason ORIGINALS_MISSING",
            ]
        );
    }
}
