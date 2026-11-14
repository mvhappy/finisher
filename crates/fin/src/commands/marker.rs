//! `fin marker add|set|remove` (plan 4C Task 23). Markers sit on `--timeline` (default the
//! first) and never ripple (Decision 26); `set` takes `label`, `color`, `at` or `duration`.

use clap::{Args, Subcommand};
use fin_core::params::ScopeOf;
use fin_core::{CoreError, MarkerAdd, MarkerRemove, MarkerSet, Op, ParamValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::track::{SetArgs, set_value};
use crate::addr;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum MarkerCmd {
    Add(AddArgs),
    Set(SetArgs),
    Remove(RemoveArgs),
}

/// Add a marker at a timeline time (a range with `dur`); binds $marker.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(long, value_name = "time")]
    pub at: String,
    #[arg(long, value_name = "s")]
    pub label: String,
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<String>,
    /// #rgb, #rrggbb or #rrggbbaa.
    #[arg(long, value_name = "#hex")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Take a marker off its timeline.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "mN")]
    pub marker: String,
}

pub fn name(cmd: &MarkerCmd) -> &'static str {
    match cmd {
        MarkerCmd::Add(_) => "marker.add",
        MarkerCmd::Set(_) => "marker.set",
        MarkerCmd::Remove(_) => "marker.remove",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &MarkerCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        MarkerCmd::Add(a) => {
            let tl = match &b.timeline {
                Some(t) => addr::timeline(&b.work, Some(&b.id(t)))?,
                None => addr::timeline(&b.work, None)?,
            };
            let (tl_id, rate) = (tl.id.clone(), tl.rate);
            let timeline = b.timeline.clone().unwrap_or(tl_id);
            let at = addr::time_at(rate, "at", &a.at)?;
            let duration = a
                .dur
                .as_deref()
                .map(|d| addr::time_at(rate, "dur", d))
                .transpose()?;
            let color = match &a.color {
                Some(c) => {
                    let spec = fin_core::params::find(&ScopeOf::Marker, "color")?;
                    match crate::values::parse(spec, "color", c, rate)? {
                        ParamValue::Text(t) => Some(t),
                        _ => None,
                    }
                }
                None => None,
            };
            let id = format!("$marker{suffix}");
            b.push(Op::MarkerAdd(MarkerAdd {
                id,
                timeline: timeline.clone(),
                at,
                duration,
                label: a.label.clone(),
                color: color.clone(),
            }))?;
            Ok(
                json!({ "timeline": timeline, "at": at, "duration": duration, "label": a.label, "color": color }),
            )
        }
        MarkerCmd::Set(a) => {
            let marker = b.id(&a.target);
            let rate = b
                .work
                .timelines
                .iter()
                .find(|tl| tl.markers.iter().any(|m| m.id == marker))
                .map_or(b.work.rate, |tl| tl.rate);
            let value = set_value(ScopeOf::Marker, a, rate)?;
            b.push(Op::MarkerSet(MarkerSet {
                marker: a.target.clone(),
                path: a.path.clone(),
                value: value.clone(),
            }))?;
            Ok(json!({ "marker": a.target, "path": a.path, "value": value }))
        }
        MarkerCmd::Remove(a) => {
            b.push(Op::MarkerRemove(MarkerRemove {
                marker: a.marker.clone(),
            }))?;
            Ok(json!({ "marker": a.marker }))
        }
    }
}
