//! `fin look --at <t>[,<t>…] [--out <dir>] [--size WxH] [--originals] [--no-wait]` (plan 6A Task 13): PNG
//! frames of a timeline, rendered by a `media.look` job on the server's media worker
//! (`media.look`). `fin` links no media crate: it resolves the times, sends the request and
//! prints the answer.
//!
//! - Times are typed as anywhere else (`12.4s`, `00:12.4`, `f300` at the timeline's rate, `62/5`)
//!   and echoed resolved (`at`, canonical rationals). `--timeline` (the global flag) picks the
//!   timeline (default the first), `--branch` the branch.
//! - Output: `{timeline, at, frames: [{at, file, width, height, plan}], skipped, adapter,
//!   stats}`. Layers a frame left out (a caption in 6A, a codec this machine lacks) are in
//!   `skipped`, each with a `code`; that is information, not a failure (exit 0, spec §2.7).
//!   A time past the timeline's end is `INVALID_RANGE`.
//! - `--no-wait` answers `{timeline, at, job}` at once; the result is the job's: `fin wait
//!   <job.id>` waits for it and `fin job show <job.id>` reads it (its `result` is the
//!   `LookResult`).
//! - A layer reads its asset's `1080` proxy when the media cache holds one (through the
//!   original's sample table: the same frame, within the proxy's quality; plan 6B-1 Task 16);
//!   `--originals` reads originals only. Each frame says which it read: `source` is `proxy` or
//!   `original`, and `proxyFallbacks` names the assets whose listed proxy could not be read.
//! - `--out` is a folder (relative to the current directory); without it the frames go to
//!   `workspace/shared/look/` of the project. It needs `project.write`.

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::output::{unavailable, usage};

/// Frames of a timeline at some times, as PNG files.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LookArgs {
    /// One or more times, comma-separated (1 to 64).
    #[arg(long, value_name = "t[,t…]", allow_hyphen_values = true)]
    pub at: String,
    /// The folder for the files (default: workspace/shared/look/ of the project).
    #[arg(long, value_name = "dir")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    /// The frame size, e.g. 960x540 (default: the timeline's resolution).
    #[arg(long, value_name = "WxH")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Read the originals only: no layer reads a proxy (default: the `1080` proxy when the
    /// cache holds one; each frame's `source` says which was read).
    #[arg(long)]
    #[serde(default)]
    pub originals: bool,
    /// Answer `{job}` at once instead of waiting for the frames.
    #[arg(long = "no-wait")]
    #[serde(default)]
    pub no_wait: bool,
}

fn bad_size(input: &str) -> CoreError {
    usage(
        format!("--size {input:?} is not WxH"),
        json!({ "input": input, "field": "size" }),
        "write the size as <width>x<height> in pixels, e.g. 960x540",
    )
}

/// `WxH`, each side a whole number (the server checks the range).
fn parse_size(input: &str) -> Result<Value, CoreError> {
    let (w, h) = input
        .split_once(['x', 'X'])
        .ok_or_else(|| bad_size(input))?;
    let side = |s: &str| s.parse::<u32>().map_err(|_| bad_size(input));
    Ok(json!({ "width": side(w)?, "height": side(h)? }))
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &LookArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": "media.look" }))
                .with_fix("fin look renders on the server: start finisherd and run it again"),
        );
    }
    let branch = target_branch(backend, globals)?;
    let snap = backend.snapshot(&branch)?;
    let tl = addr::timeline(&snap.project, globals.timeline.as_deref())?;
    let mut at = Vec::new();
    for t in a.at.split(',') {
        at.push(addr::time_at(tl.rate, "at", t.trim())?);
    }
    let mut payload = json!({ "branch": branch, "timeline": tl.id, "at": at, "wait": !a.no_wait });
    if let Some(out) = &a.out {
        payload["out"] = json!(
            crate::paths::resolve_checked_str(out)?
                .display()
                .to_string()
        );
    }
    if let Some(size) = &a.size {
        payload["size"] = parse_size(size)?;
    }
    if a.originals {
        payload["originals"] = json!(true);
    }
    let mut answer = backend.project_call("media.look", payload)?;
    if let Some(m) = answer.as_object_mut() {
        m.insert("timeline".into(), json!(tl.id));
        m.insert("at".into(), json!(at));
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_width_x_height() {
        assert_eq!(
            parse_size("960x540").unwrap(),
            json!({ "width": 960, "height": 540 })
        );
        assert_eq!(parse_size("1X2").unwrap()["height"], 2);
        for bad in ["960", "x", "960x", "0.5x1", "-1x2", "axb"] {
            assert_eq!(parse_size(bad).unwrap_err().code, "USAGE", "{bad}");
        }
    }
}
