//! `fin eval frame --at <time>` and `fin eval audio --range a..b` (plan 4C Decision 40, Task
//! 24): what the media engine draws at a time and plays over a range, evaluated on the server
//! (`eval.frame`, `eval.audio`) with its shared measurer, so captions carry layouts when their
//! pinned fonts are cached (else `layout: null` and `unmeasured`). Times are typed as
//! anywhere else (frames count at the timeline's rate) and sent as rationals. `--timeline`
//! picks the timeline (default the first), `--branch` the branch.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::output::unavailable;

#[derive(Debug, Clone, Subcommand)]
pub enum EvalCmd {
    Frame(FrameArgs),
    Audio(AudioArgs),
}

/// The frame plan at a timeline time.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameArgs {
    #[arg(long, value_name = "time")]
    pub at: String,
}

/// The audio plan over a range `a..b` of timeline time.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioArgs {
    #[arg(long, value_name = "a..b")]
    pub range: String,
}

fn bounds(rate: Rational, input: &str) -> Result<(Rational, Rational), CoreError> {
    let bad = |why: &str| {
        CoreError::new("INVALID_RANGE", format!("range {input:?} {why}"))
            .with_details(json!({ "input": input, "field": "range" }))
            .with_fix("write the range as a..b with both ends, e.g. 2s..5s")
    };
    let (a, b) = input.split_once("..").ok_or_else(|| bad("is not a..b"))?;
    if a.is_empty() || b.is_empty() {
        return Err(bad("needs both ends"));
    }
    let (from, to) = (
        addr::time_at(rate, "range", a)?,
        addr::time_at(rate, "range", b)?,
    );
    if from >= to {
        return Err(bad("is empty"));
    }
    Ok((from, to))
}

pub fn run(backend: &dyn Backend, globals: &Globals, cmd: &EvalCmd) -> Result<Value, CoreError> {
    let tag = match cmd {
        EvalCmd::Frame(_) => "eval.frame",
        EvalCmd::Audio(_) => "eval.audio",
    };
    if backend.dry_run_only() {
        return Err(unavailable(json!({ "mode": "offline", "operation": tag }))
            .with_fix("fin eval runs on the server: start finisherd and run it again"));
    }
    let branch = target_branch(backend, globals)?;
    let snap = backend.snapshot(&branch)?;
    let tl = addr::timeline(&snap.project, globals.timeline.as_deref())?;
    let mut payload = json!({ "branch": branch });
    if globals.timeline.is_some() {
        payload["timeline"] = json!(tl.id);
    }
    match cmd {
        EvalCmd::Frame(a) => payload["at"] = json!(addr::time_at(tl.rate, "at", &a.at)?),
        EvalCmd::Audio(a) => {
            let (from, to) = bounds(tl.rate, &a.range)?;
            payload["from"] = json!(from);
            payload["to"] = json!(to);
        }
    }
    let plan = backend.project_call(tag, payload)?;
    Ok(json!({ "timeline": tl.id, "plan": plan }))
}
