//! `fin track add|remove|move|set` (plan 4C Task 23). `add` acts on `--timeline` (default the
//! first); `set` takes a registry path (`name`, `mute`, `solo`, `lock`, `gain`) and a value
//! typed by it, or `--reset`.

use clap::{Args, Subcommand};
use fin_core::params::{ScopeOf, find};
use fin_core::{
    CoreError, Op, ParamValue, Rational, TrackAdd, TrackKind, TrackMove, TrackRemove, TrackSet,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::item::closed_arg;
use crate::addr;
use crate::output::usage;
use crate::values;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum TrackCmd {
    Add(AddArgs),
    Remove(RemoveArgs),
    Move(MoveArgs),
    Set(SetArgs),
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Add an empty track to `--timeline`; binds $track.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(long, value_name = "video|audio|caption")]
    pub kind: TrackKindArg,
    /// Place in compositing order, bottom first (default: on top).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Default: by kind and number (V2, A2, C1…).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

closed_arg!(
    /// A new track's `--kind`.
    TrackKindArg("kind") { Video = "video", Audio = "audio", Caption = "caption" }
);

/// Take a track off its timeline; `with-items` also removes its items (and transitions).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoveArgs {
    #[arg(value_name = "t")]
    pub track: String,
    #[arg(long = "with-items")]
    #[serde(default, skip_serializing_if = "is_false")]
    pub with_items: bool,
}

/// Move a track to another place in its timeline's compositing order (bottom first).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    #[arg(value_name = "t")]
    pub track: String,
    #[arg(long, value_name = "n")]
    pub index: u32,
}

/// Set one setting (`fin schema param` lists the paths), or reset it with `reset`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    #[arg(value_name = "id")]
    pub target: String,
    #[arg(value_name = "path")]
    pub path: String,
    #[arg(
        value_name = "value",
        allow_hyphen_values = true,
        required_unless_present = "reset",
        conflicts_with = "reset"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,
}

/// The value a `set` command sends: parsed by the path's spec in `scope`, or `None` to reset.
pub fn set_value(
    scope: ScopeOf<'_>,
    a: &SetArgs,
    rate: Rational,
) -> Result<Option<ParamValue>, CoreError> {
    let spec = find(&scope, &a.path)?;
    match (&a.value, a.reset) {
        (Some(v), false) => values::parse(spec, &a.path, v, rate).map(Some),
        (None, true) => Ok(None),
        _ => Err(value_or_reset(&a.path)),
    }
}

/// A `set` given both a value and `--reset`, or neither.
pub fn value_or_reset(path: &str) -> CoreError {
    usage(
        "give a value or --reset, not both",
        json!({ "path": path }),
        "pass <value> to set it, or --reset for its default",
    )
}

pub fn name(cmd: &TrackCmd) -> &'static str {
    match cmd {
        TrackCmd::Add(_) => "track.add",
        TrackCmd::Remove(_) => "track.remove",
        TrackCmd::Move(_) => "track.move",
        TrackCmd::Set(_) => "track.set",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &TrackCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        TrackCmd::Add(a) => {
            let timeline = match &b.timeline {
                Some(t) => b.id(t),
                None => addr::timeline(&b.work, None)?.id.clone(),
            };
            addr::timeline(&b.work, Some(&timeline))?;
            let kind = match a.kind {
                TrackKindArg::Video => TrackKind::Video,
                TrackKindArg::Audio => TrackKind::Audio,
                TrackKindArg::Caption => TrackKind::Caption,
            };
            let id = format!("$track{suffix}");
            let sent = b.timeline.clone().unwrap_or_else(|| timeline.clone());
            b.push(Op::TrackAdd(TrackAdd {
                id,
                timeline: sent.clone(),
                kind,
                index: a.index,
                name: a.name.clone(),
            }))?;
            let (tl, added) = addr::track(&b.work, &b.id(&format!("$track{suffix}")))?;
            let index = tl.tracks.iter().position(|t| t.id == added.id);
            Ok(
                json!({ "timeline": sent, "kind": a.kind.name(), "index": index, "name": added.name }),
            )
        }
        TrackCmd::Remove(a) => {
            let items = addr::track(&b.work, &b.id(&a.track)).map_or(0, |(_, t)| t.items.len());
            b.push(Op::TrackRemove(TrackRemove {
                track: a.track.clone(),
                with_items: a.with_items,
            }))?;
            Ok(json!({ "track": a.track, "withItems": a.with_items, "items": items }))
        }
        TrackCmd::Move(a) => {
            b.push(Op::TrackMove(TrackMove {
                track: a.track.clone(),
                index: a.index,
            }))?;
            Ok(json!({ "track": a.track, "index": a.index }))
        }
        TrackCmd::Set(a) => {
            let rate = addr::rate_of(&b.work, &b.id(&a.target));
            let value = set_value(ScopeOf::Track, a, rate)?;
            b.push(Op::TrackSet(TrackSet {
                track: a.target.clone(),
                path: a.path.clone(),
                value: value.clone(),
            }))?;
            Ok(json!({ "track": a.target, "path": a.path, "value": value }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review #49 follow-up: an unknown track kind is an error naming the field.
    #[test]
    fn an_unknown_kind_is_refused() {
        let e = serde_json::from_value::<AddArgs>(json!({ "kind": "vidoe" }))
            .unwrap_err()
            .to_string();
        assert!(e.contains("kind") && e.contains("caption"), "{e}");
        let ok: AddArgs = serde_json::from_value(json!({ "kind": "audio" })).unwrap();
        assert_eq!(json!(ok)["kind"], "audio");
    }
}
