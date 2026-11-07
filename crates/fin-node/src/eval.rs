//! Evaluation and pin planning on the server (4C Decisions 35, 40 and 43): `evalFrame`,
//! `evalAudio` and `fontsPlan`, short calls on the libuv pool. Each reads a branch head under
//! the store mutex and works on that copy after releasing it.
//!
//! - `evalFrame` measures captions with the server's shared measurer; a caption whose bytes
//!   are missing is unmeasured (never an error), and the measurement asks for them.
//! - `fontsPlan` answers the `font.pin` ops a client prepends to its own tx. The server never
//!   alters a client's tx. The ops are applied to a copy of the head without a measurer,
//!   so a tx that cannot apply fails here as it would at commit.

use fin_core::{CoreError, MemIds, Op, Project, Rational, SCHEMA_VERSION, Tx};
use fin_fonts::{FontCatalog, FontsPlan};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::{Value, json};

use crate::engine::{Engine, Shared, with_store};
use crate::tasks::{Call, Reply, StoreTask};
use crate::{encode, reject};

fn time(name: &str, s: &str) -> Result<Rational> {
    s.parse::<Rational>().map_err(|e: CoreError| {
        reject(
            CoreError::new(
                "INVALID_TIME",
                format!("{name} is not a time: {}", e.message),
            )
            .with_details(json!({ "field": name, "value": s }))
            .with_fix("send a rational number of seconds such as \"12\" or \"1001/30000\""),
        )
    })
}

/// The head of `branch` (resolved by id, name or `<name>#<id>`).
fn head(store: &Shared, branch: &str) -> Result<(u64, Project)> {
    with_store(store, |s| s.snapshot(branch).map_err(reject))
}

/// `timeline`, else the project's main (first) timeline.
fn timeline_id(project: &Project, timeline: Option<String>) -> Result<String> {
    timeline
        .or_else(|| project.timelines.first().map(|t| t.id.clone()))
        .ok_or_else(|| reject(CoreError::new("NOT_FOUND", "the project has no timeline")))
}

pub(crate) fn run(store: &Shared, call: Call) -> Result<Reply> {
    match call {
        Call::EvalFrame {
            branch,
            timeline,
            at,
        } => {
            let at = time("at", &at)?;
            let (_, project) = head(store, &branch)?;
            let timeline = timeline_id(&project, timeline)?;
            let m = store.fonts.manager().measurer();
            let plan =
                fin_core::eval::eval_frame(&project, &timeline, at, Some(&*m)).map_err(reject)?;
            Ok(Reply::Json(encode(&plan)?))
        }
        Call::EvalAudio {
            branch,
            timeline,
            from,
            to,
        } => {
            let (from, to) = (time("from", &from)?, time("to", &to)?);
            let (_, project) = head(store, &branch)?;
            let timeline = timeline_id(&project, timeline)?;
            let plan = fin_core::eval::eval_audio(&project, &timeline, from, to).map_err(reject)?;
            Ok(Reply::Json(encode(&plan)?))
        }
        Call::FontsPlan { branch, ops_json } => {
            let ops: Vec<Op> = serde_json::from_str(&ops_json).map_err(|e| {
                reject(
                    CoreError::new("INVALID_OP", format!("ops do not decode: {e}"))
                        .with_details(json!({ "reason": e.to_string() }))
                        .with_fix("send the ops of the tx about to be committed"),
                )
            })?;
            let (rev, project) = head(store, &branch)?;
            let manager = store.fonts.manager();
            let planned = plan(&project, &branch, rev, ops, manager.catalog()).map_err(reject)?;
            // Decision 34: planned files are fetched at once (in the background).
            let value = serde_json::to_value(&planned).unwrap_or(Value::Null);
            manager.request(planned_shas(&value).into_iter());
            Ok(Reply::Json(encode(&planned)?))
        }
        _ => unreachable!("not an eval call"),
    }
}

/// The SHA-256 of each `font.pin` op's pin.
fn planned_shas(plan: &Value) -> Vec<&str> {
    plan["ops"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|op| op["op"] == "font.pin")
        .filter_map(|op| op["pin"]["sha256"].as_str())
        .collect()
}

/// Decision 35: the client's ops, applied to a copy of the head without a measurer (so a tx
/// that cannot apply fails as it would at commit), then the pins its fonts need.
///
/// As built in Task 22 the model has no caption styles or `font.*` ops yet (Tasks 18–19), so
/// nothing needs a pin and the plan is empty. Task 20 replaces the empty plan with
/// `fin_fonts::plan::plan_pins(head, &tx.ops, catalog)`.
fn plan(
    head: &Project,
    branch: &str,
    rev: u64,
    ops: Vec<Op>,
    _catalog: &FontCatalog,
) -> std::result::Result<FontsPlan, CoreError> {
    let tx = fin_core::normalize(Tx {
        tx_id: "fonts.plan".to_string(),
        author: "fonts.plan".to_string(),
        branch: branch.to_string(),
        base_rev: rev,
        schema_version: SCHEMA_VERSION,
        ops,
        args_hash: None,
    })?;
    fin_core::apply_tx(head, &tx, &mut MemIds::after(head))?;
    Ok(FontsPlan {
        ops: Vec::new(),
        notes: Vec::new(),
    })
}

#[napi]
impl Engine {
    /// Resolves to the `FramePlan` JSON of `timeline` (default: the main timeline) on
    /// `branch` at `at` (a rational time such as `"12"` or `"1001/30000"`). Captions are
    /// measured with the server's shared measurer; missing bytes leave them unmeasured.
    #[napi(catch_unwind)]
    pub fn eval_frame(
        &self,
        branch: String,
        timeline: Option<String>,
        at: String,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::EvalFrame {
            branch,
            timeline,
            at,
        })
    }

    /// Resolves to the `AudioPlan` JSON of `timeline` (default: the main one) on `branch`
    /// over `[from, to)`.
    #[napi(catch_unwind)]
    pub fn eval_audio(
        &self,
        branch: String,
        timeline: Option<String>,
        from: String,
        to: String,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::EvalAudio {
            branch,
            timeline,
            from,
            to,
        })
    }

    /// Resolves to the `FontsPlan` JSON (`{ops, notes}`) for the ops (JSON array) about to be
    /// committed on `branch`: the `font.pin` ops to prepend. Planned files are fetched.
    #[napi(catch_unwind)]
    pub fn fonts_plan(&self, branch: String, ops_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::FontsPlan { branch, ops_json })
    }
}
