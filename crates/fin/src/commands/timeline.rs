//! `fin timeline add|set|remove` (plan 4C Task 23). A new timeline takes the project rate and
//! 1080×1920 unless told otherwise; `set` takes `name`, `resolution` (WxH) or `rate` (only
//! while it has no items).

use clap::{Args, Subcommand};
use fin_core::params::ScopeOf;
use fin_core::{CoreError, Op, Resolution, TimelineAdd, TimelineRemove, TimelineSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::track::{SetArgs, set_value};
use crate::addr;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum TimelineCmd {
    Add(AddArgs),
    Set(SetArgs),
    Remove(RemoveArgs),
}

/// Add an empty timeline; binds $timeline.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(long, value_name = "s")]
    pub name: String,
    /// Frames per second (n, n/d or a decimal); default the project rate.
    #[arg(long, value_name = "fps")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// Frame size WxH; default 1080x1920.
    #[arg(long, value_name = "WxH")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// Take a timeline off the project with everything on it (not the last one, nor one a nested
/// item shows).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "tl")]
    pub timeline: String,
}

pub fn name(cmd: &TimelineCmd) -> &'static str {
    match cmd {
        TimelineCmd::Add(_) => "timeline.add",
        TimelineCmd::Set(_) => "timeline.set",
        TimelineCmd::Remove(_) => "timeline.remove",
    }
}

fn resolution(input: &str) -> Result<Resolution, CoreError> {
    let bad = || {
        usage(
            format!("--resolution {input:?} is not WxH"),
            json!({ "field": "resolution", "input": input }),
            "write the frame size as WxH, e.g. --resolution 1920x1080",
        )
    };
    let (w, h) = input.split_once(['x', 'X']).ok_or_else(bad)?;
    Ok(Resolution {
        width: w.trim().parse().map_err(|_| bad())?,
        height: h.trim().parse().map_err(|_| bad())?,
    })
}

pub fn plan(b: &mut Builder<'_>, cmd: &TimelineCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        TimelineCmd::Add(a) => {
            let rate = match &a.rate {
                Some(r) => {
                    let spec = fin_core::params::find(&ScopeOf::Timeline, "rate")?;
                    let v = crate::values::parse(spec, "rate", r, b.work.rate)?;
                    let fin_core::ParamValue::Text(text) = v else {
                        unreachable!("rates are text")
                    };
                    text.parse()?
                }
                None => b.work.rate,
            };
            let resolution = match &a.resolution {
                Some(r) => resolution(r)?,
                None => Resolution {
                    width: 1080,
                    height: 1920,
                },
            };
            let id = format!("$timeline{suffix}");
            b.push(Op::TimelineAdd(TimelineAdd {
                id,
                name: a.name.clone(),
                rate,
                resolution,
            }))?;
            Ok(json!({ "name": a.name, "rate": rate, "resolution": resolution }))
        }
        TimelineCmd::Set(a) => {
            let rate =
                addr::timeline(&b.work, Some(&b.id(&a.target))).map_or(b.work.rate, |t| t.rate);
            let value = set_value(ScopeOf::Timeline, a, rate)?;
            b.push(Op::TimelineSet(TimelineSet {
                timeline: a.target.clone(),
                path: a.path.clone(),
                value: value.clone(),
            }))?;
            Ok(json!({ "timeline": a.target, "path": a.path, "value": value }))
        }
        TimelineCmd::Remove(a) => {
            b.push(Op::TimelineRemove(TimelineRemove {
                timeline: a.timeline.clone(),
            }))?;
            Ok(json!({ "timeline": a.timeline }))
        }
    }
}
