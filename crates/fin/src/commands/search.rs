//! `fin library search` and `fin search` (plan 5D Task 14, spec §7.6): names and folders across
//! the server's libraries, ranked, one `library.search` RPC (`library.read`). Offline it is
//! `SERVER_UNAVAILABLE`.
//!
//! `fin search [<query>] [--lib <lib>]… [--kind <k>] [--min-duration <t>] [--max-duration <t>]
//! [--min-width <px>] [--min-height <px>] [--since <date>] [--state <s>] [--limit <n>] [--cursor
//! <c>]` answers `{hits, next?, truncated, stale, lastPollAt?}`: pass `next` as `--cursor` for the
//! next page (pages share a snapshot of up to 2,000 hits, so they never repeat or skip). With no
//! query it lists by last change. `--lib` is a library's id or name (repeatable; default: every
//! library this principal reads). Durations are times (`90s`, `1:30`, `12.4`); `--since` is
//! `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ` (UTC) or epoch milliseconds.

use clap::Args;
use fin_core::{CoreError, Rational, parse_time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::diagnostics::parse_date;
use super::library::{FILE_STATES, offline};
use crate::backend::Backend;
use crate::output::usage;

/// The most hits a page holds.
pub const SEARCH_LIMIT_MAX: u32 = 200;

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Words to find in names and folders (prefixes match; omit to list by last change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Only this library (id or name; repeatable).
    #[arg(long, value_name = "lib")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lib: Vec<String>,
    /// Only this kind of file (video, audio, image, …).
    #[arg(long, value_name = "k")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// At least this long (a time such as 90s or 1:30).
    #[arg(long = "min-duration", value_name = "t")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_duration: Option<String>,
    /// At most this long.
    #[arg(long = "max-duration", value_name = "t")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration: Option<String>,
    /// At least this wide, in pixels.
    #[arg(long = "min-width", value_name = "px")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<u32>,
    /// At least this tall, in pixels.
    #[arg(long = "min-height", value_name = "px")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<u32>,
    /// Changed since this date (YYYY-MM-DD, YYYY-MM-DDTHH:MM:SSZ or epoch milliseconds).
    #[arg(long, value_name = "date")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Only files in this state (present, missing, cloudOnly, unreadable, probeFailed, skipped,
    /// offline).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// At most this many hits (1 to 200, default 50).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next` of the previous page.
    #[arg(long, value_name = "c")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A duration flag as whole milliseconds: a time at rate 1 (`90s`, `1:30`); a bare number is seconds.
fn duration_ms(flag: &str, text: &str) -> Result<u64, CoreError> {
    let bad = || {
        usage(
            format!("{flag} {text:?} is not a duration"),
            json!({ "flag": flag.trim_start_matches('-') }),
            "use seconds (12.4), 12.4s, 1:30 or 01:00:12.4",
        )
    };
    let t = text.trim();
    let spelled = if t.parse::<f64>().is_ok() {
        format!("{t}s")
    } else {
        t.to_string()
    };
    let secs = parse_time(&spelled, Rational::from_int(1))
        .map_err(|_| bad())?
        .to_f64();
    let ms = (secs * 1000.0).round();
    if !ms.is_finite() || !(0.0..=9.0e15).contains(&ms) {
        return Err(bad());
    }
    Ok(ms as u64)
}

/// The `library.search` payload of `a`; flags are checked before anything is sent.
pub fn payload(a: &SearchArgs) -> Result<Value, CoreError> {
    let mut p = json!({});
    if let Some(q) = &a.query {
        p["query"] = json!(q);
    }
    if !a.lib.is_empty() {
        p["libraries"] = json!(a.lib);
    }
    if let Some(k) = &a.kind {
        p["kind"] = json!(k);
    }
    let range = |min: Option<u64>, max: Option<u64>| {
        let mut r = json!({});
        if let Some(n) = min {
            r["min"] = json!(n);
        }
        if let Some(n) = max {
            r["max"] = json!(n);
        }
        r
    };
    let min = a
        .min_duration
        .as_deref()
        .map(|t| duration_ms("--min-duration", t))
        .transpose()?;
    let max = a
        .max_duration
        .as_deref()
        .map(|t| duration_ms("--max-duration", t))
        .transpose()?;
    if let (Some(lo), Some(hi)) = (min, max)
        && lo > hi
    {
        return Err(usage(
            "--min-duration is longer than --max-duration",
            json!({ "min": lo, "max": hi }),
            "give the shorter duration to --min-duration",
        ));
    }
    if min.is_some() || max.is_some() {
        p["duration"] = range(min, max);
    }
    if let Some(w) = a.min_width {
        p["width"] = range(Some(w.into()), None);
    }
    if let Some(h) = a.min_height {
        p["height"] = range(Some(h.into()), None);
    }
    if let Some(s) = &a.since {
        p["since"] = json!(parse_date("--since", s)?);
    }
    if let Some(s) = &a.state {
        if !FILE_STATES.contains(&s.as_str()) {
            return Err(usage(
                format!("--state {s:?} is not a file state"),
                json!({ "state": s, "states": FILE_STATES }),
                format!("use one of {}", FILE_STATES.join(", ")),
            ));
        }
        p["state"] = json!(s);
    }
    if let Some(n) = a.limit {
        if !(1..=SEARCH_LIMIT_MAX).contains(&n) {
            return Err(usage(
                format!("--limit {n} is outside 1 to {SEARCH_LIMIT_MAX}"),
                json!({ "limit": n, "max": SEARCH_LIMIT_MAX }),
                format!("ask for at most {SEARCH_LIMIT_MAX} hits and page with --cursor"),
            ));
        }
        p["limit"] = json!(n);
    }
    if let Some(c) = &a.cursor {
        p["cursor"] = json!(c);
    }
    Ok(p)
}

pub fn run(backend: &dyn Backend, a: &SearchArgs) -> Result<Value, CoreError> {
    let p = payload(a)?;
    if backend.dry_run_only() {
        return Err(offline("library.search"));
    }
    backend.server_call("library.search", p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(f: impl FnOnce(&mut SearchArgs)) -> SearchArgs {
        let mut a = SearchArgs {
            query: None,
            lib: vec![],
            kind: None,
            min_duration: None,
            max_duration: None,
            min_width: None,
            min_height: None,
            since: None,
            state: None,
            limit: None,
            cursor: None,
        };
        f(&mut a);
        a
    }

    #[test]
    fn flags_become_the_rpcs_payload() {
        assert_eq!(payload(&args(|_| {})).unwrap(), json!({}));
        let p = payload(&args(|a| {
            a.query = Some("interview".into());
            a.lib = vec!["raw".into(), "01abc".into()];
            a.kind = Some("video".into());
            a.min_duration = Some("1:30".into());
            a.max_duration = Some("5:00".into());
            a.min_width = Some(1920);
            a.min_height = Some(1080);
            a.since = Some("2026-11-01".into());
            a.state = Some("cloudOnly".into());
            a.limit = Some(20);
            a.cursor = Some("c1".into());
        }))
        .unwrap();
        assert_eq!(
            p,
            json!({
                "query": "interview", "libraries": ["raw", "01abc"], "kind": "video",
                "duration": { "min": 90_000, "max": 300_000 }, "width": { "min": 1920 },
                "height": { "min": 1080 }, "since": 1_793_491_200_000u64, "state": "cloudOnly",
                "limit": 20, "cursor": "c1",
            })
        );
    }

    #[test]
    fn bad_flags_are_usage_errors_before_anything_is_sent() {
        type Edit = Box<dyn FnOnce(&mut SearchArgs)>;
        let bad: [Edit; 5] = [
            Box::new(|a| a.min_duration = Some("soon".into())),
            Box::new(|a| a.since = Some("yesterday".into())),
            Box::new(|a| a.state = Some("gone".into())),
            Box::new(|a| a.limit = Some(201)),
            Box::new(|a| {
                a.min_duration = Some("10".into());
                a.max_duration = Some("5".into());
            }),
        ];
        for f in bad {
            assert_eq!(payload(&args(f)).unwrap_err().code, "USAGE");
        }
        assert_eq!(
            payload(&args(|a| a.limit = Some(0))).unwrap_err().code,
            "USAGE"
        );
    }
}
