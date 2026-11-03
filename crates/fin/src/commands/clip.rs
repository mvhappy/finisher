//! `fin clip insert|trim|delete`.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Edge, Op, Rational};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Builder;
use crate::addr;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand)]
pub enum ClipCmd {
    Insert(InsertArgs),
    Trim(TrimArgs),
    Delete(DeleteArgs),
}

/// Insert a clip, rippling later clips on the track. Give exactly one of `dur` and `out`.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InsertArgs {
    /// Track id, e.g. "t1".
    #[arg(long, value_name = "t")]
    pub track: String,
    /// Position: a time ("12.4s", "00:12.4", "f300", "62/5"), "end" (track end) or
    /// "after:<clip>" (that clip's end).
    #[arg(long, value_name = "time|after:cN|end", allow_hyphen_values = true)]
    pub at: String,
    /// Asset id ("a1"), or in a batch a placeholder bound by an earlier line ("$asset1").
    #[arg(long, value_name = "aN")]
    pub asset: String,
    /// Offset into the asset where the clip starts; default 0.
    #[arg(long = "in", value_name = "time")]
    #[serde(rename = "in", default)]
    pub source_in: Option<String>,
    /// Clip length.
    #[arg(long, value_name = "time", required_unless_present = "out", conflicts_with = "out")]
    #[serde(default)]
    pub dur: Option<String>,
    /// Asset time where the clip ends; length = out - in.
    #[arg(long, value_name = "time")]
    #[serde(default)]
    pub out: Option<String>,
}

/// Move one edge of a clip by a signed delta, rippling later clips. Give exactly one of
/// `start` and `end`.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrimArgs {
    /// Clip id, e.g. "c3".
    #[arg(value_name = "cN")]
    pub clip: String,
    /// Signed delta for the start edge: "+0.5s" trims in, "-1s" extends.
    #[arg(long, value_name = "±time", allow_hyphen_values = true, required_unless_present = "end", conflicts_with = "end")]
    #[serde(default)]
    pub start: Option<String>,
    /// Signed delta for the end edge: "+1s" extends, "-0.5s" trims in.
    #[arg(long, value_name = "±time", allow_hyphen_values = true)]
    #[serde(default)]
    pub end: Option<String>,
}

/// Delete a clip and close the gap (ripple).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// Clip id, e.g. "c3".
    #[arg(value_name = "cN")]
    pub clip: String,
}

/// Exactly one of two optional flags.
fn one_of<'v>(cmd: &str, a: (&str, &'v Option<String>), b: (&str, &'v Option<String>)) -> Result<(bool, &'v str), CoreError> {
    match (a.1, b.1) {
        (Some(v), None) => Ok((true, v)),
        (None, Some(v)) => Ok((false, v)),
        _ => Err(usage(
            format!("{cmd} needs exactly one of {} and {}", a.0, b.0),
            json!({ "alternatives": [a.0, b.0] }),
            format!("pass either --{} or --{}", a.0, b.0),
        )),
    }
}

pub fn plan_insert(b: &mut Builder<'_>, a: &InsertArgs, placeholder: &str) -> Result<Value, CoreError> {
    let (by_dur, length) = one_of("clip.insert", ("dur", &a.dur), ("out", &a.out))?;
    let p = &b.work;
    let track = b.id(&a.track);
    let at_input = match a.at.strip_prefix("after:") {
        Some(clip) => format!("after:{}", b.id(clip)),
        None => a.at.clone(),
    };
    let at = addr::position(p, &track, "at", &at_input)?;
    let source_in = match &a.source_in {
        Some(s) => addr::time(p, "in", s)?,
        None => Rational::zero(),
    };
    let duration = if by_dur {
        addr::time(p, "dur", length)?
    } else {
        addr::time(p, "out", length)?.checked_sub(source_in)?
    };
    let out = source_in.checked_add(duration)?;
    let resolved = json!({
        "track": a.track,
        "at": at,
        "asset": a.asset,
        "in": source_in,
        "duration": duration,
        "out": out,
    });
    b.push(Op::ClipInsert {
        id: placeholder.to_string(),
        track: a.track.clone(),
        at,
        asset: a.asset.clone(),
        source_in,
        duration,
    })?;
    Ok(resolved)
}

pub fn plan_trim(b: &mut Builder<'_>, a: &TrimArgs) -> Result<Value, CoreError> {
    let (is_start, input) = one_of("clip.trim", ("start", &a.start), ("end", &a.end))?;
    let (edge, field) = if is_start { (Edge::Start, "start") } else { (Edge::End, "end") };
    let delta = addr::delta(&b.work, field, input)?;
    b.push(Op::ClipTrim { clip: a.clip.clone(), edge, delta })?;
    Ok(json!({ "clip": a.clip, "edge": edge, "delta": delta }))
}

pub fn plan_delete(b: &mut Builder<'_>, a: &DeleteArgs) -> Result<Value, CoreError> {
    b.push(Op::ClipDelete { clip: a.clip.clone() })?;
    Ok(json!({ "clip": a.clip }))
}
