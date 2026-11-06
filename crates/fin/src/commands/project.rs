//! `fin project create|open|status|takeover|verify|rebuild|compact|recover|acknowledge`.
//! All need the server, except `create --dry-run` (validated locally) and `rebuild` while no
//! server runs (done in-process). `open` only registers the folder with the server: it is not
//! a write, so `--dry-run` does not apply to it; nor does it to the storage commands, which
//! never change the edit history.

use clap::{Args, Subcommand};
use fin_core::{CoreError, Rational, frame_duration, new_project};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Globals;
use crate::backend::Backend;
use crate::paths;

#[derive(Debug, Clone, Subcommand)]
pub enum ProjectCmd {
    Create(CreateArgs),
    Open(OpenArgs),
    Status(NoArgs),
    Takeover(TakeoverArgs),
    Verify(NoArgs),
    Rebuild(NoArgs),
    Compact(NoArgs),
    Recover(RecoverArgs),
    Acknowledge(AcknowledgeArgs),
}

/// No input besides the global flags.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

/// List the groups of set-aside records; make recovered branches of them (`--create`) and read
/// segment files moved out of the folder into the quarantine first (`--import`).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecoverArgs {
    /// Make a `recovered-*` branch of one group (its 12-hex id) or of every recoverable one
    /// (`all`). Without it the groups are only listed.
    #[arg(long, value_name = "group|all")]
    #[serde(default)]
    pub create: Option<String>,
    /// A folder of `.fseg` files moved out of the project folder (a forked or conflicted
    /// chain), read into the quarantine before listing.
    #[arg(long, value_name = "dir")]
    #[serde(default)]
    pub import: Option<String>,
}

/// Take the project's lease over from a server that stopped.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TakeoverArgs {
    /// Say the other server really is stopped: needed on sync folders, which never take over
    /// on their own.
    #[arg(long)]
    #[serde(default)]
    pub confirm: bool,
}

/// Stop `fin project status` warning about a rewind or an incomplete salvage you have dealt
/// with. Exactly one of --rewind and --salvage.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcknowledgeArgs {
    /// The generation a rewind started (the REWOUND / SPLIT_BRAIN_SUSPECTED warning's
    /// `details.generation`).
    #[arg(
        long,
        value_name = "n",
        conflicts_with = "salvage",
        required_unless_present = "salvage"
    )]
    #[serde(default)]
    pub rewind: Option<u64>,
    /// The kept database copy of a SALVAGE_INCOMPLETE warning (`details.copy`).
    #[arg(long, value_name = "copy")]
    #[serde(default)]
    pub salvage: Option<String>,
}

/// Create a project folder with a video track t1 and an audio track t2.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// Folder to create the project in.
    #[arg(value_name = "dir")]
    pub dir: String,
    /// Project name.
    #[arg(long, value_name = "s")]
    pub name: String,
    /// Timeline frame rate in frames per second, as an integer or rational ("30", "30000/1001").
    #[arg(long, value_name = "fps", default_value = "30")]
    #[serde(default)]
    pub rate: Option<String>,
}

/// Open an existing project folder: registers it with the server. Not a write, so --dry-run
/// does not apply (it is accepted and ignored).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    /// Project folder.
    #[arg(value_name = "dir")]
    pub dir: String,
    /// The project now lives here, although the folder it was recorded at cannot be reached
    /// (PROJECT_ORIGINAL_UNREACHABLE). Only when you know it moved: a copy opened this way
    /// takes the original's place on this server.
    #[arg(long)]
    #[serde(default)]
    pub relocate: bool,
}

pub fn create(
    backend: &dyn Backend,
    globals: &Globals,
    a: &CreateArgs,
) -> Result<Value, CoreError> {
    let rate_input = a.rate.as_deref().unwrap_or("30");
    let rate: Rational = rate_input.parse().map_err(|mut e: CoreError| {
        // A rational in another form than its canonical one keeps the fix naming that form.
        let canonical = e.details.get("canonical").cloned();
        e.details = json!({ "input": rate_input, "field": "rate" });
        match canonical {
            Some(c) => e.details["canonical"] = c,
            None => {
                e.fix = Some("give the rate as frames per second: 30, 25, 24000/1001".to_string())
            }
        }
        e
    })?;
    frame_duration(rate)?;
    fin_store::validate_name(&a.name)?;
    let idem = match globals.idem_key.as_deref() {
        Some(key) if key.trim().is_empty() => {
            return Err(crate::output::usage(
                "--idem-key must not be empty",
                json!({ "field": "idem-key" }),
                "pass a non-empty key",
            ));
        }
        key => key,
    };
    let dir = paths::resolve_str(&a.dir);
    let resolved = json!({ "dir": dir.display().to_string(), "name": a.name, "rate": rate });
    if globals.dry_run || backend.dry_run_only() {
        let project = new_project("$project".to_string(), a.name.clone(), rate)?;
        return Ok(json!({ "dryRun": true, "project": project, "resolved": resolved }));
    }
    // With a key, a create whose answer was lost (timeout, dropped connection) is sent once
    // more: the server answers a retry with the project the first one made.
    let project = match backend.create_project(&dir, &a.name, rate, idem) {
        Err(e) if idem.is_some() && crate::client::IN_FLIGHT_CODES.contains(&e.code.as_str()) => {
            backend.create_project(&dir, &a.name, rate, idem)?
        }
        other => other?,
    };
    Ok(json!({ "project": project, "resolved": resolved }))
}

pub fn open(backend: &dyn Backend, a: &OpenArgs) -> Result<Value, CoreError> {
    let dir = paths::resolve_str(&a.dir);
    let project = backend.open_project(&dir, a.relocate)?;
    Ok(json!({ "project": project, "resolved": { "dir": dir.display().to_string() } }))
}

/// `VERIFY_FAILED` (exit 3): `details.failed` names the failed checks.
fn verify_failed(report: Value) -> CoreError {
    let failed: Vec<Value> = report["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["ok"] == json!(false))
        .map(|c| c["name"].clone())
        .collect();
    let names: Vec<&str> = failed.iter().filter_map(Value::as_str).collect();
    CoreError::new(
        "VERIFY_FAILED",
        format!("the project failed verification: {}", names.join(", ")),
    )
    .with_details(json!({ "report": report, "failed": failed }))
    .with_fix("fin project rebuild rebuilds the database from the folder; if segment files are damaged, restore them from a backup first (the report names them)")
}

/// `fin project recover`: the report, less the written records (`written` counts them) so a
/// recovery of many records stays one screen; every other field is passed through as is.
fn recover(backend: &dyn Backend, g: &Globals, a: &RecoverArgs) -> Result<Value, CoreError> {
    let mut fields = json!({});
    if let Some(create) = &a.create {
        fields["create"] = json!(create);
    }
    if let Some(dir) = &a.import {
        fields["importDir"] = json!(paths::resolve_str(dir).display().to_string());
    }
    let args = json!({ "create": a.create, "import": a.import });
    let req = super::history::Req {
        tag: "project.recover",
        cmd: "project.recover",
        args,
        fields,
        hashed: false,
        group: true,
    };
    let mut report = super::history::send(backend, g, req)?;
    if let Some(map) = report.as_object_mut() {
        let written = map.remove("committed");
        map.insert(
            "written".into(),
            json!(
                written
                    .as_ref()
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            ),
        );
    }
    Ok(report)
}

/// The storage commands (`status` … `acknowledge`). `create` and `open` are handled above.
pub fn storage(backend: &dyn Backend, g: &Globals, cmd: &ProjectCmd) -> Result<Value, CoreError> {
    let call = |tag: &str, payload: Value| backend.project_call(tag, payload);
    Ok(match cmd {
        ProjectCmd::Status(_) => json!({ "status": call("project.status", json!({}))? }),
        ProjectCmd::Takeover(a) => {
            json!({ "status": call("project.takeover", json!({ "confirm": a.confirm }))? })
        }
        ProjectCmd::Verify(_) => {
            let report = call("project.verify", json!({}))?;
            if report["ok"] == json!(false) {
                return Err(verify_failed(report));
            }
            json!({ "report": report })
        }
        ProjectCmd::Rebuild(_) => {
            let (report, offline) = backend.rebuild()?;
            json!({ "report": report, "offline": offline })
        }
        ProjectCmd::Compact(_) => json!({ "report": call("project.compact", json!({}))? }),
        ProjectCmd::Recover(a) => recover(backend, g, a)?,
        ProjectCmd::Acknowledge(a) => match (&a.rewind, &a.salvage) {
            (Some(n), None) => {
                let branch = super::target_branch(backend, g)?;
                call(
                    "project.acknowledgeRewind",
                    json!({ "branch": branch, "generation": n }),
                )?
            }
            (None, Some(copy)) => call("project.acknowledgeSalvage", json!({ "copy": copy }))?,
            _ => {
                return Err(crate::output::usage(
                    "give exactly one of --rewind <n> and --salvage <copy>",
                    json!({ "field": "rewind" }),
                    "fin project acknowledge --rewind <n> [--branch <b>] | --salvage <copy>",
                ));
            }
        },
        ProjectCmd::Create(_) | ProjectCmd::Open(_) => unreachable!("handled by create/open"),
    })
}
