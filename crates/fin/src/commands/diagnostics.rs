//! `fin diagnostics show|export|purge|project|consent` (plan D, Decisions 14a, 14b, 30 and 33;
//! spec §21.1). The journal is the server's: every command is one `diagnostics.*` RPC, so
//! offline it is `SERVER_UNAVAILABLE`. `export` prints JSONL, newest last, paged by the
//! server's `next` (at most 1000 lines a page); the rest print one JSON document. When an
//! export fails after it printed lines (a later page), its error document is the last line of
//! stdout and the exit code is the error's: entries are objects with `v` and `id`, the error is
//! `{"error": …}`. An export that fails on its first page prints only the error.
//!
//! `--project` is the global flag: a project folder (as everywhere) or, here, a project id.
//! Opting in to content is a person's choice: `consent training on` is a usage error that names
//! the ways in, and the server refuses it from this socket as well (`CONSENT_UI_ONLY`). The
//! server serves `fin`, which is not a person (5A Task 7), redacted exports, and refuses consent
//! changes, purges, `project default` and `export --content` with `HUMAN_ONLY` (exit 7): those
//! are the owner's, in the Finisher app or the server's console.

use clap::{Args, Subcommand, ValueEnum};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Globals;
use crate::backend::Backend;
use crate::output::{self, usage};

#[derive(Debug, Clone, Subcommand)]
pub enum DiagnosticsCmd {
    Show(ShowArgs),
    Export(ExportArgs),
    Purge(PurgeArgs),
    Project(ProjectArgs),
    Consent(ConsentArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Diagnostics,
    Training,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Purpose::Diagnostics => "diagnostics",
            Purpose::Training => "training",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Switch {
    On,
    Off,
    Unset,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Collect {
    Never,
    Default,
}

/// The consent, the journal's state and counts per kind and project (`export --limit n` lists
/// the newest entries).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowArgs {}

/// JSONL of what is stored, oldest first, with content removed (`--content` asks for it, which
/// only a person may), or with `--as-upload` exactly what an uploader would get now.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    /// Entries from this time on: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ` or epoch milliseconds (UTC).
    #[arg(long, value_name = "date")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Only these kinds (repeat the flag).
    #[arg(long, value_name = "k")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kind: Vec<String>,
    /// Keep the newest n entries.
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The view an uploader would ship for this purpose now (closed files, current consent).
    #[arg(long = "as-upload", value_name = "purpose")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_upload: Option<Purpose>,
    /// Include recorded content (human-only: refused to `fin`).
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub content: bool,
    /// Which store: the diagnostics journal (default) or the agent-trace store (`agentTraces`;
    /// metadata only here, as `--content` stays human-only).
    #[arg(long, value_name = "store")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<Store>,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize, Serialize, JsonSchema)]
pub enum Store {
    #[value(name = "diagnostics")]
    #[serde(rename = "diagnostics")]
    Diagnostics,
    #[value(name = "agentTraces")]
    #[serde(rename = "agentTraces")]
    AgentTraces,
}

impl Store {
    fn as_str(self) -> &'static str {
        match self {
            Store::Diagnostics => "diagnostics",
            Store::AgentTraces => "agentTraces",
        }
    }
}

/// Deletes recorded entries: `--all` (also the key and every blob), `--project <id>` or
/// `--before <date>`. Exactly one.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PurgeArgs {
    #[arg(long)]
    #[serde(default)]
    pub all: bool,
    /// Entries before this time: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ` or epoch milliseconds (UTC).
    #[arg(long, value_name = "date")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
}

/// `never`: record nothing about this project (its entries are purged); `default`: follow each
/// user's settings again. Writes `finisher.project.json`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectArgs {
    #[arg(value_name = "never|default")]
    pub collect: Collect,
}

/// `consent diagnostics on|off|unset`; `consent training off|unset`. `unset` removes the key
/// from `settings.json`: diagnostics follow the default, training is `undecided` again.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsentArgs {
    #[arg(value_name = "diagnostics|training")]
    pub purpose: Purpose,
    #[arg(value_name = "on|off|unset")]
    pub value: Switch,
}

pub fn run(backend: &dyn Backend, g: &Globals, cmd: &DiagnosticsCmd) -> Result<Value, CoreError> {
    // `--dry-run` is a global flag, and these change the journal or settings for real: it is
    // refused, never silently ignored (as `fin project open` refuses it).
    if g.dry_run
        && let Some(name) = mutating(cmd)
    {
        return Err(usage(
            format!("diagnostics {name} is not a project write and takes no --dry-run"),
            json!({ "command": format!("diagnostics.{name}"), "flag": "dry-run" }),
            format!("run fin diagnostics {name} without --dry-run (it acts at once)"),
        ));
    }
    match cmd {
        DiagnosticsCmd::Show(a) => show(backend, a),
        DiagnosticsCmd::Export(a) => export(backend, g, a),
        DiagnosticsCmd::Purge(a) => purge(backend, g, a),
        DiagnosticsCmd::Project(a) => project(backend, g, a),
        DiagnosticsCmd::Consent(a) => consent(backend, a),
    }
}

/// The subcommands that delete or change something.
fn mutating(cmd: &DiagnosticsCmd) -> Option<&'static str> {
    match cmd {
        DiagnosticsCmd::Purge(_) => Some("purge"),
        DiagnosticsCmd::Project(_) => Some("project"),
        DiagnosticsCmd::Consent(_) => Some("consent"),
        DiagnosticsCmd::Show(_) | DiagnosticsCmd::Export(_) => None,
    }
}

// ---- show -------------------------------------------------------------------------------

fn show(backend: &dyn Backend, _: &ShowArgs) -> Result<Value, CoreError> {
    let mut report = backend.server_call("diagnostics.status", json!({}))?;
    if report["state"] == "suspended" {
        return Err(suspended(&report));
    }
    if let Some(env) = report.pointer("/consent/env").cloned() {
        report["env"] = env;
    }
    Ok(report)
}

// ---- export -----------------------------------------------------------------------------

fn export_page(
    backend: &dyn Backend,
    filter: &Value,
    cursor: Option<&str>,
    a: &ExportArgs,
    limit: u32,
) -> Result<Value, CoreError> {
    let mut payload = json!({ "filter": filter, "limit": limit });
    if a.content {
        payload["content"] = json!(true);
    }
    if let Some(s) = a.store {
        payload["store"] = json!(s.as_str());
    }
    if let Some(c) = cursor {
        payload["cursor"] = json!(c);
    }
    if let Some(p) = a.as_upload {
        payload["asUpload"] = json!(p.as_str());
    }
    backend.server_call("diagnostics.export", payload)
}

fn export(backend: &dyn Backend, g: &Globals, a: &ExportArgs) -> Result<Value, CoreError> {
    let mut filter = json!({});
    if let Some(since) = &a.since {
        filter["since"] = json!(parse_date("--since", since)?);
    }
    if let Some(project) = project_arg(backend, g, false)? {
        filter["project"] = json!(project);
    }
    if !a.kind.is_empty() {
        filter["kinds"] = json!(a.kind);
    }
    if let Some(n) = a.limit {
        filter["limit"] = json!(n);
    }
    output::streaming();
    let mut cursor: Option<String> = None;
    loop {
        let page = export_page(backend, &filter, cursor.as_deref(), a, 1000)?;
        for line in page["lines"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            output::stream_line(line);
        }
        match page["next"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return Ok(Value::Null),
        }
    }
}

// ---- purge, project, consent --------------------------------------------------------------

fn purge(backend: &dyn Backend, g: &Globals, a: &PurgeArgs) -> Result<Value, CoreError> {
    let selected =
        usize::from(a.all) + usize::from(a.before.is_some()) + usize::from(g.project.is_some());
    if selected != 1 {
        let (message, fix) = if selected == 0 {
            (
                "purge needs a selector",
                "pass --all, --project <id> or --before <date>",
            )
        } else {
            (
                "purge takes one selector",
                "pass only one of --all, --project <id> and --before <date>",
            )
        };
        return Err(usage(
            message,
            json!({ "command": "diagnostics.purge" }),
            fix,
        ));
    }
    let selector = if a.all {
        json!("all")
    } else if let Some(before) = &a.before {
        json!({ "before": parse_date("--before", before)? })
    } else {
        let id = project_arg(backend, g, true)?;
        json!({ "project": id })
    };
    backend.server_call("diagnostics.purge", json!({ "selector": selector }))
}

fn project(backend: &dyn Backend, g: &Globals, a: &ProjectArgs) -> Result<Value, CoreError> {
    let id = project_arg(backend, g, true)?;
    let collect = match a.collect {
        Collect::Never => "never",
        Collect::Default => "default",
    };
    backend.server_call(
        "diagnostics.project",
        json!({ "projectId": id, "collect": collect }),
    )
}

/// The journal is suspended: `JOURNAL_UNAVAILABLE`, its `fix` naming the reason (`report.code`).
fn suspended(report: &Value) -> CoreError {
    let code = report["code"].as_str().unwrap_or("IO");
    let fix = match code {
        "JOURNAL_UNWRITABLE" => {
            "make <server-data>/events a writable directory (and not a file), then restart finisherd"
        }
        "DISK_FULL" => "free disk space where the server data lives; the journal retries by itself",
        "PANIC" => "restart finisherd; the journal's writer stopped after an internal error",
        _ => "check that <server-data>/events can be written, then restart finisherd",
    };
    CoreError::new(
        "JOURNAL_UNAVAILABLE",
        format!("the diagnostics journal is suspended ({code})"),
    )
    .with_details(json!({ "reason": code, "report": report }))
    .with_fix(fix)
}

/// What `consent training on` says: a person opts in, a command does not.
fn training_on_refused() -> CoreError {
    usage(
        "turning training on is a person's choice, not a command's",
        json!({
            "purpose": "training",
            "value": "on",
            "ways": ["the Finisher app (onboarding)", "settings.json privacy.training", "FIN_TRAINING=on"],
        }),
        "a person opts in: in the Finisher app, by setting privacy.training to \"on\" in <server-data>/settings.json by hand, or by starting finisherd with FIN_TRAINING=on",
    )
}

fn consent(backend: &dyn Backend, a: &ConsentArgs) -> Result<Value, CoreError> {
    if matches!((a.purpose, a.value), (Purpose::Training, Switch::On)) {
        return Err(training_on_refused());
    }
    let value = match a.value {
        Switch::On => "on",
        Switch::Off => "off",
        Switch::Unset => "unset",
    };
    backend.server_call(
        "diagnostics.consent",
        json!({ "purpose": a.purpose.as_str(), "value": value }),
    )
}

// ---- arguments --------------------------------------------------------------------------------

/// A Crockford-base32 ULID: 26 characters of `0-9A-Z` without `I L O U`, the first at most `7`
/// (the 48-bit time must not overflow). Ids from the server are lower case.
fn looks_like_id(s: &str) -> bool {
    let s = s.as_bytes();
    s.len() == 26
        && s[0] <= b'7'
        && s.iter()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_alphabetic() && !b"ILOUilou".contains(c)))
}

/// The project `--project` names: an id (a valid ULID that is no folder here), else the
/// folder's project as the server knows it; without the flag, the nearest project when
/// `required`.
fn project_arg(
    backend: &dyn Backend,
    g: &Globals,
    required: bool,
) -> Result<Option<String>, CoreError> {
    match &g.project {
        Some(p) if looks_like_id(&p.to_string_lossy()) && !p.exists() => {
            Ok(Some(p.to_string_lossy().to_ascii_lowercase()))
        }
        Some(_) => backend.project_id().map(Some),
        None if required => backend.project_id().map(Some),
        None => Ok(None),
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn bad_date(flag: &str, text: &str) -> CoreError {
    usage(
        format!("{flag} {text:?} is not a date"),
        json!({ "flag": flag.trim_start_matches('-') }),
        "use YYYY-MM-DD, YYYY-MM-DDTHH:MM:SSZ (UTC) or epoch milliseconds",
    )
}

/// `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM[:SS[.fff]]Z` (UTC) or a number of epoch milliseconds, as
/// milliseconds since the epoch.
pub fn parse_date(flag: &str, text: &str) -> Result<u64, CoreError> {
    let bad = || bad_date(flag, text);
    if !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()) {
        return text.parse().map_err(|_| bad());
    }
    let (date, time) = match text.split_once('T') {
        Some((d, t)) => (d, Some(t.strip_suffix('Z').ok_or_else(bad)?)),
        None => (text, None),
    };
    let num = |s: &str, len: usize| -> Result<i64, CoreError> {
        if s.len() == len && s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().map_err(|_| bad())
        } else {
            Err(bad())
        }
    };
    let mut ymd = date.split('-');
    let (y, m, d) = match (ymd.next(), ymd.next(), ymd.next(), ymd.next()) {
        (Some(y), Some(m), Some(d), None) => (num(y, 4)?, num(m, 2)?, num(d, 2)?),
        _ => return Err(bad()),
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&m) || d < 1 || d > month_days[m as usize - 1] {
        return Err(bad());
    }
    let mut ms = days_from_civil(y, m, d) * 86_400_000;
    if let Some(t) = time {
        let (hms, frac) = t.split_once('.').map_or((t, None), |(a, b)| (a, Some(b)));
        let mut p = hms.split(':');
        let (h, mi, s) = match (p.next(), p.next(), p.next(), p.next()) {
            (Some(h), Some(mi), s, None) => {
                (num(h, 2)?, num(mi, 2)?, s.map(|s| num(s, 2)).transpose()?)
            }
            _ => return Err(bad()),
        };
        let frac_ms = match frac {
            Some(f) if (1..=9).contains(&f.len()) && f.bytes().all(|c| c.is_ascii_digit()) => {
                let padded = format!("{f:0<3}");
                padded[..3].parse::<i64>().map_err(|_| bad())?
            }
            Some(_) => return Err(bad()),
            None => 0,
        };
        if h > 23 || mi > 59 || s.unwrap_or(0) > 59 {
            return Err(bad());
        }
        ms += ((h * 60 + mi) * 60 + s.unwrap_or(0)) * 1000 + frac_ms;
    }
    u64::try_from(ms).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_milliseconds() {
        let ok = |s: &str| parse_date("--since", s).unwrap();
        assert_eq!(ok("1970-01-01"), 0);
        assert_eq!(ok("2026-11-07"), 1_794_009_600_000);
        assert_eq!(ok("2026-11-07T00:00:01Z"), 1_794_009_601_000);
        assert_eq!(ok("2026-11-07T01:02:03.5Z"), 1_794_009_600_000 + 3_723_500);
        assert_eq!(ok("2026-11-07T01:02Z"), 1_794_009_600_000 + 3_720_000);
        assert_eq!(ok("1794009600000"), 1_794_009_600_000);
        assert_eq!(ok("2024-02-29"), 1_709_164_800_000);
        for bad in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-02-29",
            "2026-11-31",
            "26-11-07",
            "2026-11-07T10:00",
            "2026-11-07T25:00:00Z",
            "1969-12-31",
            "2026-11-07T10:00:00.Z",
        ] {
            assert_eq!(
                parse_date("--since", bad).unwrap_err().code,
                "USAGE",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn only_a_valid_ulid_is_a_project_id() {
        for ok in [
            "01hzzzzzzzzzzzzzzzzzzzzzzz",
            "01HZXG3Q4TWV5N6M7K8J9A0B1C",
            "7zzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert!(looks_like_id(ok), "{ok}");
        }
        for bad in [
            "",
            "01hzzzzzzzzzzzzzzzzzzzzzz",
            "01hzzzzzzzzzzzzzzzzzzzzzzzz",
            "81hzzzzzzzzzzzzzzzzzzzzzzz",
            "01hzzzzzzzzzzzzzzzzzzzzzzi",
            "01hzzzzzzzzzzzzzzzzzzzzzzl",
            "01hzzzzzzzzzzzzzzzzzzzzzzo",
            "01hzzzzzzzzzzzzzzzzzzzzzzu",
            "film/folderfolderfolder12",
        ] {
            assert!(!looks_like_id(bad), "{bad}");
        }
    }
}
