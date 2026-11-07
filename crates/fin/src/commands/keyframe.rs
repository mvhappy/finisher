//! `fin keyframe set|remove` (plan 4C Task 24). `--at` is relative to the item's start
//! (Decision 13: keyframes are content-anchored); the value is typed by the path's spec. The
//! easing presets resolve to CSS `cubic-bezier` curves (Decision 27), echoed as sent.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Interp, KeyframeRemove, KeyframeSet, Op};
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

/// Set the keyframe of an animatable path at `at` (item-relative).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "path")]
    pub path: String,
    /// Item-relative time.
    #[arg(long, value_name = "time")]
    pub at: String,
    #[arg(value_name = "value", allow_hyphen_values = true)]
    pub value: String,
    /// hold, linear (default), ease, easeIn, easeOut, easeInOut or bezier:x1,y1,x2,y2.
    #[arg(long, value_name = "interp")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interp: Option<String>,
}

/// Remove the keyframe of a path at `at` (item-relative).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "path")]
    pub path: String,
    #[arg(long, value_name = "time")]
    pub at: String,
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
            let item = b.id(&a.item);
            let (spec, rate) = item_spec(&b.work, &item, &a.path)?;
            let at = addr::time_at(rate, "at", &a.at)?;
            let value = values::parse(spec, &a.path, &a.value, rate)?;
            let interp = interp(a.interp.as_deref().unwrap_or("linear"))?;
            b.push(Op::from(KeyframeSet {
                item: a.item.clone(),
                path: a.path.clone(),
                at,
                value: value.clone(),
                interp,
            }))?;
            Ok(
                json!({ "item": a.item, "path": a.path, "at": at, "value": value, "interp": interp }),
            )
        }
        KeyframeCmd::Remove(a) => {
            let item = b.id(&a.item);
            let (_, rate) = item_spec(&b.work, &item, &a.path)?;
            let at = addr::time_at(rate, "at", &a.at)?;
            b.push(Op::from(KeyframeRemove {
                item: a.item.clone(),
                path: a.path.clone(),
                at,
            }))?;
            Ok(json!({ "item": a.item, "path": a.path, "at": at }))
        }
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
