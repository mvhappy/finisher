//! `fin clip insert|trim|delete`: clip edits, sent as `item.*` ops (plan 4C Tasks 7 and 23; the
//! `clip.*` op aliases are for older clients). An insert of an asset with audio on a video
//! track also places a linked audio clip (`compose::clip_with_audio`, Decision 21) unless
//! `--no-audio`; an overwrite strictly inside an item splits it first (`compose::overwrite`).
//!
//! New flags are left out of the command input when absent, so an equivalent 4B call hashes
//! the same (`argsHash`, Decision 7).

use clap::{Args, Subcommand};
use fin_core::compose::{clip_with_audio, overwrite};
use fin_core::{CoreError, Edge, InsertMode, ItemBody, ItemDelete, ItemInsert, ItemTrim, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::Builder;
use super::builder::rename_json;
use super::item::{
    DeleteModeArg, InsertModeArg, TrimModeArg, delete_mode, delete_name, edge_at, insert_mode,
    mode_name, trim_mode, trim_name,
};
use crate::addr;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand)]
pub enum ClipCmd {
    Insert(InsertArgs),
    Trim(TrimArgs),
    Delete(DeleteArgs),
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Insert a clip (ripple by default). Give exactly one of `dur` and `out`. An asset with audio
/// on a video track also gets a linked audio clip on `audioTrack` (default: the timeline's
/// first audio track) unless `noAudio`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
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
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub source_in: Option<String>,
    /// Clip length.
    #[arg(
        long,
        value_name = "time",
        required_unless_present = "out",
        conflicts_with = "out"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<String>,
    /// Asset time where the clip ends; length = out - in.
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    /// "ripple" (default: later items move) or "overwrite" (takes the range over).
    #[arg(long, value_name = "ripple|overwrite")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<InsertModeArg>,
    /// Insert the video alone, without its linked audio.
    #[arg(long = "no-audio", conflicts_with = "audio_track")]
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_audio: bool,
    /// Audio track for the linked audio; default the timeline's first audio track.
    #[arg(long = "audio-track", value_name = "t")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_track: Option<String>,
}

/// Move one edge of a clip by a signed delta (ripple by default). Give exactly one of `start`
/// and `end`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrimArgs {
    /// Clip id, e.g. "c3".
    #[arg(value_name = "cN")]
    pub clip: String,
    /// Signed delta for the start edge: "+0.5s" trims in, "-1s" extends.
    #[arg(
        long,
        value_name = "±time",
        allow_hyphen_values = true,
        required_unless_present = "end",
        conflicts_with = "end"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Signed delta for the end edge: "+1s" extends, "-0.5s" trims in.
    #[arg(long, value_name = "±time", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// "ripple" (default), "roll" (moves the cut with the touching neighbour) or "overwrite"
    /// (the edge alone: carves what it extends over, leaves a gap when it shrinks).
    #[arg(long, value_name = "ripple|roll|overwrite")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TrimModeArg>,
}

/// Delete a clip: "ripple" (default) closes the gap, "lift" leaves it.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    /// Clip id, e.g. "c3".
    #[arg(value_name = "cN")]
    pub clip: String,
    /// "ripple" (default) or "lift".
    #[arg(long, value_name = "ripple|lift")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<DeleteModeArg>,
}

/// Exactly one of two optional flags.
pub(super) fn one_of<'v>(
    cmd: &str,
    a: (&str, &'v Option<String>),
    b: (&str, &'v Option<String>),
) -> Result<(bool, &'v str), CoreError> {
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

/// `--out` at or before `--in` (SP2 follow-up: the wording names both values).
fn out_before_in(source_in: Rational, out: Rational) -> CoreError {
    CoreError::new(
        "INVALID_RANGE",
        format!("--out must be after --in (--in {source_in}s, --out {out}s)"),
    )
    .with_details(json!({ "field": "out", "in": source_in, "out": out }))
    .with_fix(format!(
        "give an --out later than {source_in}s, or the length with --dur"
    ))
}

/// `--in` at or past the asset's end (SP2 follow-up: no negative `max` to puzzle over).
fn in_past_end(asset: &str, source_in: Rational, length: Rational) -> CoreError {
    CoreError::new(
        "INVALID_RANGE",
        format!("sourceIn is at or past the asset's end: --in {source_in}s, asset {asset} is {length}s long"),
    )
    .with_details(json!({ "field": "in", "sourceIn": source_in, "assetDuration": length, "asset": asset }))
    .with_fix(format!(
        "use an --in below {length}s, the duration of asset {asset}"
    ))
}

pub fn plan_insert(b: &mut Builder<'_>, a: &InsertArgs, suffix: &str) -> Result<Value, CoreError> {
    let (by_dur, length) = one_of("clip.insert", ("dur", &a.dur), ("out", &a.out))?;
    let mode = insert_mode(a.mode, InsertMode::Ripple);
    let track = b.id(&a.track);
    let asset = b.id(&a.asset);
    let p = &b.work;
    let rate = addr::rate_of(p, &track);
    let at_input = match a.at.strip_prefix("after:") {
        Some(clip) => format!("after:{}", b.id(clip)),
        None => a.at.clone(),
    };
    let at = addr::position(p, &track, "at", &at_input)?;
    let source_in = match &a.source_in {
        Some(s) => addr::time_at(rate, "in", s)?,
        None => Rational::zero(),
    };
    let duration = if by_dur {
        addr::time_at(rate, "dur", length)?
    } else {
        let out = addr::time_at(rate, "out", length)?;
        if out <= source_in {
            return Err(out_before_in(source_in, out));
        }
        out.checked_sub(source_in)?
    };
    if let Some(len) = p.assets.get(&asset).and_then(|x| x.duration)
        && source_in >= len
    {
        return Err(in_past_end(&a.asset, source_in, len));
    }
    let out = source_in.checked_add(duration)?;
    let mut resolved = json!({
        "track": a.track,
        "at": at,
        "asset": a.asset,
        "in": source_in,
        "duration": duration,
        "out": out,
        "mode": mode_name(mode),
    });
    let one = Rational::from_int(1);
    let composed = if a.no_audio {
        let body = ItemBody::Clip {
            asset,
            source_in,
            speed: one,
        };
        match mode {
            InsertMode::Overwrite => overwrite(p, &track, at, duration, body, "$clip")?,
            InsertMode::Ripple => fin_core::compose::Composed {
                ops: vec![
                    ItemInsert {
                        id: "$clip".into(),
                        track: track.clone(),
                        at,
                        duration,
                        mode,
                        body,
                    }
                    .into(),
                ],
                echo: json!({}),
            },
        }
    } else {
        let audio_track = a.audio_track.as_ref().map(|t| b.id(t));
        clip_with_audio(
            p,
            &track,
            at,
            duration,
            &asset,
            source_in,
            one,
            mode,
            audio_track.as_deref(),
        )?
    };
    let names = b.push_composed(composed.ops, suffix)?;
    let mut echo = composed.echo;
    rename_json(&mut echo, &names);
    merge_echo(&mut resolved, echo);
    Ok(resolved)
}

/// Adds a builder's echo fields that say something (`audio`, `link`, `split(s)`).
pub(super) fn merge_echo(resolved: &mut Value, echo: Value) {
    let (Some(map), Value::Object(echo)) = (resolved.as_object_mut(), echo) else {
        return;
    };
    let echo: Map<String, Value> = echo;
    for (k, v) in echo {
        let empty = v.is_null() || v.as_array().is_some_and(Vec::is_empty);
        if !empty && !map.contains_key(&k) {
            map.insert(k, v);
        }
    }
}

pub fn plan_trim(b: &mut Builder<'_>, a: &TrimArgs) -> Result<Value, CoreError> {
    let (is_start, input) = one_of("clip.trim", ("start", &a.start), ("end", &a.end))?;
    let (edge, field) = if is_start {
        (Edge::Start, "start")
    } else {
        (Edge::End, "end")
    };
    let item = b.id(&a.clip);
    let rate = addr::item(&b.work, &item).map_or(b.work.rate, |(tl, _, _)| tl.rate);
    let delta = addr::delta_at(rate, field, input)?;
    let mode = trim_mode(a.mode, true);
    b.push(
        ItemTrim {
            item: a.clip.clone(),
            edge,
            delta,
            mode,
            alone: false,
        }
        .into(),
    )?;
    Ok(json!({
        "clip": a.clip,
        "edge": edge,
        "delta": delta,
        "mode": trim_name(mode),
        "edgeAt": edge_at(&b.work, &item, edge)?,
    }))
}

pub fn plan_delete(b: &mut Builder<'_>, a: &DeleteArgs) -> Result<Value, CoreError> {
    let mode = delete_mode(a.mode, true);
    b.push(
        ItemDelete {
            item: a.clip.clone(),
            mode,
        }
        .into(),
    )?;
    Ok(json!({ "clip": a.clip, "mode": delete_name(mode) }))
}
