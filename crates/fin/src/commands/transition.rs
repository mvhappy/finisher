//! `fin transition set|remove`: transitions at cuts, centred on the cut (plan 4C Decision 22).
//! `set` also crossfades the cut's touching link partners (`compose::transition_with_links`;
//! `--no-linked` opts out) and updates a transition a cut already has instead of refusing
//! `TRANSITION_EXISTS`.

use clap::{Args, Subcommand};
use fin_core::compose::transition_with_links;
use fin_core::{CoreError, Curve, Op, TrackKind, TransitionKind, TransitionRemove};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::builder::rename_json;
use super::item::closed_arg;
use crate::addr;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum TransitionCmd {
    Set(SetArgs),
    Remove(RemoveArgs),
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Set the transition at the cut between two touching items (outgoing first); binds
/// $transition (and $transition2… for link partners).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SetArgs {
    #[arg(long, value_names = ["a", "b"], num_args = 2, required = true)]
    pub between: Vec<String>,
    /// "dissolve" (video) or "crossfade" (audio); default by the track's kind.
    #[arg(long, value_name = "dissolve|crossfade")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TransitionKindArg>,
    /// Length, centred on the cut.
    #[arg(long, value_name = "time")]
    pub dur: String,
    /// "linear" (default) or "equalPower".
    #[arg(long, value_name = "linear|equalPower")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<CurveArg>,
    /// Leave the link partners' cuts alone.
    #[arg(long = "no-linked")]
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_linked: bool,
}

closed_arg!(
    /// A transition's `--kind`.
    TransitionKindArg("kind") { Dissolve = "dissolve", Crossfade = "crossfade" }
);
closed_arg!(
    /// A transition's `--curve`.
    CurveArg("curve") { Linear = "linear", EqualPower = "equalPower" }
);

/// Take a transition off its track.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "trN")]
    pub transition: String,
}

pub fn name(cmd: &TransitionCmd) -> &'static str {
    match cmd {
        TransitionCmd::Set(_) => "transition.set",
        TransitionCmd::Remove(_) => "transition.remove",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &TransitionCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        TransitionCmd::Set(a) => plan_set(b, a, suffix),
        TransitionCmd::Remove(a) => {
            b.push(Op::TransitionRemove(TransitionRemove {
                transition: a.transition.clone(),
            }))?;
            Ok(json!({ "transition": a.transition }))
        }
    }
}

fn plan_set(b: &mut Builder<'_>, a: &SetArgs, suffix: &str) -> Result<Value, CoreError> {
    let out = b.id(&a.between[0]);
    let inc = b.id(&a.between[1]);
    let (tl, track, _) = addr::item(&b.work, &out)?;
    let kind = match a.kind {
        Some(TransitionKindArg::Crossfade) => TransitionKind::Crossfade,
        Some(TransitionKindArg::Dissolve) => TransitionKind::Dissolve,
        None if track.kind == TrackKind::Audio => TransitionKind::Crossfade,
        None => TransitionKind::Dissolve,
    };
    let curve_arg = a.curve.unwrap_or(CurveArg::Linear);
    let curve = match curve_arg {
        CurveArg::Linear => Curve::Linear,
        CurveArg::EqualPower => Curve::EqualPower,
    };
    let duration = addr::time_at(tl.rate, "dur", &a.dur)?;
    let mut composed =
        transition_with_links(&b.work, &out, &inc, kind, duration, curve, !a.no_linked)?;
    // A cut that has a transition gets it updated (its literal id), not a second one.
    let mut updated = Vec::new();
    for op in &mut composed.ops {
        let Op::TransitionSet(set) = op else { continue };
        let existing = addr::track(&b.work, &set.track).ok().and_then(|(_, t)| {
            t.transitions
                .iter()
                .find(|x| x.between == set.between)
                .map(|x| x.id.clone())
        });
        if let Some(id) = existing {
            updated.push(json!({ "placeholder": set.id, "id": id }));
            set.id = id;
        }
    }
    let names = b.push_composed(composed.ops, suffix)?;
    let mut echo = composed.echo;
    for u in &updated {
        let (Some(p), Some(id)) = (u["placeholder"].as_str(), u["id"].as_str()) else {
            continue;
        };
        if let Some(list) = echo["transitions"].as_array_mut() {
            for t in list.iter_mut().filter(|t| t["id"] == p) {
                t["id"] = json!(id);
                t["updated"] = json!(true);
            }
        }
    }
    rename_json(&mut echo, &names);
    let kind_name = match kind {
        TransitionKind::Crossfade => "crossfade",
        TransitionKind::Dissolve => "dissolve",
    };
    let mut resolved = json!({
        "between": a.between, "kind": kind_name, "duration": duration,
        "curve": curve_arg.name(),
    });
    super::clip::merge_echo(&mut resolved, echo);
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review #49 follow-up: an unknown kind or curve is an error naming the field, never a
    /// silent default.
    #[test]
    fn unknown_kinds_and_curves_are_refused() {
        let args = |extra: Value| {
            let mut a = json!({ "between": ["c1", "c2"], "dur": "1s" });
            a.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::from_value::<SetArgs>(a)
        };
        let e = args(json!({ "curve": "equalpower" }))
            .unwrap_err()
            .to_string();
        assert!(e.contains("curve") && e.contains("equalPower"), "{e}");
        let e = args(json!({ "kind": "wipe" })).unwrap_err().to_string();
        assert!(e.contains("kind") && e.contains("crossfade"), "{e}");
        let ok = args(json!({ "kind": "crossfade", "curve": "equalPower" })).unwrap();
        assert_eq!(json!(ok)["curve"], "equalPower");
    }
}
