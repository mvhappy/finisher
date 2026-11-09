//! `fin keyframe set|remove` (plan 4C Task 24). `--at` is relative to the item's start
//! (Decision 13: keyframes are content-anchored); the value is typed by the path's spec. The
//! easing presets resolve to CSS `cubic-bezier` curves (Decision 27), echoed as sent. A key
//! is named by time (`--at`) or by id (`--key kf3`, spec §5.12.3); every write echoes the
//! key's id (an insert's is the one the commit minted, [`echo_key`]).

use clap::{Args, Subcommand};
use fin_core::{
    CommittedTx, CoreError, Interp, KeyframeRemove, KeyframeSet, Op, Project, Rational,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::param::item_spec;
use crate::addr;
use crate::output::usage;
use crate::values;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum KeyframeCmd {
    Set(SetArgs),
    Remove(RemoveArgs),
}

/// Set the keyframe of an animatable path at `at` (item-relative), or the key `key` (moved
/// to `at` when given).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "path")]
    pub path: String,
    /// Item-relative time.
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The key's id, e.g. kf3.
    #[arg(long, value_name = "kfN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[arg(value_name = "value", allow_hyphen_values = true)]
    pub value: String,
    /// hold, linear (default), ease, easeIn, easeOut, easeInOut or bezier:x1,y1,x2,y2.
    #[arg(long, value_name = "interp")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interp: Option<String>,
}

/// Remove the keyframe of a path at `at` (item-relative) or the key `key`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "path")]
    pub path: String,
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The key's id, e.g. kf3.
    #[arg(long, value_name = "kfN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The id and time of a key of `item`'s `path` on `project`: the one at `at`, else `key`.
fn find_key(
    project: &Project,
    item: &str,
    path: &str,
    key: Option<&str>,
    at: Option<Rational>,
) -> Option<(String, Rational)> {
    let (_, _, it) = addr::item(project, item).ok()?;
    let keys = it.common.keyframes.get(path)?;
    let k = match key {
        Some(id) => keys.iter().find(|k| k.id == id),
        None => keys.iter().find(|k| Some(k.at) == at),
    }?;
    Some((k.id.clone(), k.at))
}

fn needs_key(cmd: &str, both: bool) -> CoreError {
    let message = if both {
        format!("keyframe {cmd} takes one of --at and --key, not both")
    } else {
        format!("keyframe {cmd} needs --at or --key")
    };
    usage(
        message,
        json!({ "fields": ["at", "key"] }),
        "name the key by time (--at 2s) or by id (--key kf3)",
    )
}

const PRESETS: &[(&str, [f64; 4])] = &[
    ("ease", [0.25, 0.1, 0.25, 1.0]),
    ("easeIn", [0.42, 0.0, 1.0, 1.0]),
    ("easeOut", [0.0, 0.0, 0.58, 1.0]),
    ("easeInOut", [0.42, 0.0, 0.58, 1.0]),
];

fn bezier([x1, y1, x2, y2]: [f64; 4]) -> Interp {
    Interp::Bezier { x1, y1, x2, y2 }
}

/// An `--interp` value: `hold`, `linear`, a preset, or `bezier:x1,y1,x2,y2`.
pub fn interp(input: &str) -> Result<Interp, CoreError> {
    let bad = || {
        usage(
            format!("--interp {input:?} is not an interpolation"),
            json!({ "interp": input, "alternatives": ["hold", "linear", "ease", "easeIn", "easeOut", "easeInOut", "bezier:x1,y1,x2,y2"] }),
            "use hold, linear, ease, easeIn, easeOut, easeInOut or bezier:0.4,0,0.2,1",
        )
    };
    match input {
        "hold" => return Ok(Interp::Hold),
        "linear" => return Ok(Interp::Linear),
        _ => {}
    }
    if let Some((_, c)) = PRESETS.iter().find(|(n, _)| *n == input) {
        return Ok(bezier(*c));
    }
    let body = input.strip_prefix("bezier:").ok_or_else(bad)?;
    let nums: Vec<f64> = body
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    let [x1, y1, x2, y2] = nums.as_slice() else {
        return Err(bad());
    };
    if !nums.iter().all(|n| n.is_finite()) {
        return Err(bad());
    }
    Ok(bezier([*x1, *y1, *x2, *y2]))
}

pub fn name(cmd: &KeyframeCmd) -> &'static str {
    match cmd {
        KeyframeCmd::Set(_) => "keyframe.set",
        KeyframeCmd::Remove(_) => "keyframe.remove",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &KeyframeCmd) -> Result<Value, CoreError> {
    match cmd {
        KeyframeCmd::Set(a) => {
            if a.at.is_none() && a.key.is_none() {
                return Err(needs_key("set", false));
            }
            let item = b.id(&a.item);
            let (spec, rate) = item_spec(&b.work, &item, &a.path)?;
            let at =
                a.at.as_deref()
                    .map(|t| addr::time_at(rate, "at", t))
                    .transpose()?;
            let value = values::parse(spec, &a.path, &a.value, rate)?;
            let interp = interp(a.interp.as_deref().unwrap_or("linear"))?;
            b.push(Op::from(KeyframeSet {
                item: a.item.clone(),
                path: a.path.clone(),
                key: a.key.clone(),
                at,
                value: value.clone(),
                interp,
            }))?;
            // On the working copy the key is where the op put it (an insert under the id a dry
            // run mints; a commit's own id replaces it).
            let found = match &a.key {
                Some(k) => find_key(&b.work, &item, &a.path, Some(&b.id(k)), None),
                None => find_key(&b.work, &item, &a.path, None, at),
            };
            let key = found
                .as_ref()
                .map_or_else(|| json!(a.key), |(id, _)| json!(id));
            let at = at.or(found.map(|(_, t)| t));
            Ok(
                json!({ "item": a.item, "path": a.path, "key": key, "at": at, "value": value,
                       "interp": interp }),
            )
        }
        KeyframeCmd::Remove(a) => {
            if a.at.is_some() == a.key.is_some() {
                return Err(needs_key("remove", a.at.is_some()));
            }
            let item = b.id(&a.item);
            let (_, rate) = item_spec(&b.work, &item, &a.path)?;
            let at =
                a.at.as_deref()
                    .map(|t| addr::time_at(rate, "at", t))
                    .transpose()?;
            // Looked up before the removal: the echo names the key and its time.
            let found = find_key(&b.work, &item, &a.path, a.key.as_deref(), at);
            b.push(Op::from(KeyframeRemove {
                item: a.item.clone(),
                path: a.path.clone(),
                key: a.key.clone(),
                at,
            }))?;
            let key = found
                .as_ref()
                .map_or_else(|| json!(a.key), |(id, _)| json!(id));
            let at = at.or(found.map(|(_, t)| t));
            Ok(json!({ "item": a.item, "path": a.path, "key": key, "at": at }))
        }
    }
}

/// A committed keyframe write's `resolved.key`: the key its committed op names (the server
/// stores the key form, spec §5.12.3), an insert's placeholder (`$kf0`, or `--key $k`) read
/// through the commit's bindings. So an insert echoes the id the server minted, not the one
/// the working copy guessed (the server never reuses the id of a removed key).
pub(super) fn echo_key(resolved: &mut Value, committed: &CommittedTx) {
    let Some(map) = resolved.as_object_mut() else {
        return;
    };
    if !map.contains_key("key") {
        return;
    }
    let (Some(item), Some(path)) = (map.get("item"), map.get("path")) else {
        return;
    };
    let key = committed.tx.ops.iter().find_map(|op| match op {
        Op::KeyframeSet(k) if json!(k.item) == *item && json!(k.path) == *path => k.key.as_ref(),
        Op::KeyframeRemove(k) if json!(k.item) == *item && json!(k.path) == *path => k.key.as_ref(),
        _ => None,
    });
    if let Some(key) = key {
        let id = committed.bindings.get(key).unwrap_or(key);
        map.insert("key".into(), json!(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_presets_resolve_to_bezier() {
        assert_eq!(interp("hold").unwrap(), Interp::Hold);
        assert_eq!(interp("easeOut").unwrap(), bezier([0.0, 0.0, 0.58, 1.0]));
        assert_eq!(
            interp("bezier:0.4,0,0.2,1").unwrap(),
            bezier([0.4, 0.0, 0.2, 1.0])
        );
        for bad in [
            "bouncy",
            "bezier:1,2,3",
            "bezier:a,b,c,d",
            "bezier:inf,0,0,0",
        ] {
            assert_eq!(interp(bad).unwrap_err().code, "USAGE", "{bad}");
        }
    }
}
