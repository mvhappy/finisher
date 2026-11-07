//! Short engine calls: napi `AsyncTask`s on the libuv pool, holding the store mutex briefly.

use fin_core::{CoreError, Tx};
use fin_store::{Generation, TxStatus};
use napi::bindgen_prelude::*;
use serde_json::json;

use crate::engine::{Shared, State, lock, with_store};
use crate::{encode, guarded, reject};

pub(crate) enum Call {
    Head {
        branch: String,
    },
    Commit {
        tx_json: String,
        generation_json: Option<String>,
        meta_json: Option<String>,
    },
    Snapshot {
        branch: String,
    },
    OpsSince {
        branch: String,
        after: f64,
    },
    OpsSinceAt {
        branch: String,
        after: f64,
    },
    Status,
    TxGet {
        tx_id: String,
    },
    AcknowledgeRewind {
        branch: String,
        generation: f64,
    },
    AcknowledgeSalvage {
        copy: String,
    },
    // The branch and history calls (`crate::history`); each `req_json` is the camelCase JSON
    // of the fin-store request, each `meta_json` the caller's actor (`log.commit`).
    BranchCreate {
        req_json: String,
        meta_json: Option<String>,
    },
    Branches {
        all: bool,
    },
    BranchSet {
        req_json: String,
        meta_json: Option<String>,
    },
    ResolveBranch {
        name: String,
    },
    Undo {
        req_json: String,
        meta_json: Option<String>,
    },
    Redo {
        req_json: String,
        meta_json: Option<String>,
    },
    Checkpoint {
        req_json: String,
        meta_json: Option<String>,
    },
    Merge {
        req_json: String,
        meta_json: Option<String>,
    },
    CherryPick {
        req_json: String,
        meta_json: Option<String>,
    },
    Revert {
        req_json: String,
        meta_json: Option<String>,
    },
    // Evaluation and pin planning (`crate::eval`).
    EvalFrame {
        branch: String,
        timeline: Option<String>,
        at: String,
    },
    EvalAudio {
        branch: String,
        timeline: Option<String>,
        from: String,
        to: String,
    },
    FontsPlan {
        branch: String,
        ops_json: String,
    },
    /// `setCollect`: the project's collection flag (`default` or `never`).
    SetCollect {
        collect: String,
    },
    /// Test hook: panics while holding the store.
    #[cfg(feature = "test-hooks")]
    Panic,
}

/// What a `StoreTask` hands back to JS.
pub enum Reply {
    Number(u64),
    Json(String),
    Bool(bool),
    Null,
}

pub struct StoreTask {
    store: Shared,
    call: Option<Call>,
}

impl StoreTask {
    pub(crate) fn new(store: Shared, call: Call) -> StoreTask {
        StoreTask {
            store,
            call: Some(call),
        }
    }
}

/// A client's tx. Decoding normalises rationals (stored data is read leniently), so times
/// not in canonical form are refused here: `INVALID_TIME` naming the canonical form.
fn parse_tx(json: &str) -> Result<Tx> {
    let bad = |e: serde_json::Error| {
        reject(
            CoreError::new("INVALID_OP", format!("transaction does not decode: {e}"))
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix("send a transaction matching the fin-core Tx schema"),
        )
    };
    let input: serde_json::Value = serde_json::from_str(json).map_err(bad)?;
    let tx: Tx = serde_json::from_value(input.clone()).map_err(bad)?;
    let decoded = serde_json::to_value(&tx).map_err(bad)?;
    fin_core::check_canonical_times(&input, &decoded).map_err(reject)?;
    Ok(tx)
}

fn parse_generation(json: &str) -> Result<Generation> {
    serde_json::from_str(json).map_err(|e| {
        reject(
            CoreError::new("INVALID_OP", format!("generation does not decode: {e}"))
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix("send the generation {incarnation, n} as opsSinceAt or status gave it"),
        )
    })
}

fn parse_count(name: &str, n: f64) -> Result<u64> {
    // 2^53: the largest integer a JS number holds exactly. NaN fails both checks.
    if n.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&n) {
        Ok(n as u64)
    } else {
        Err(reject(
            CoreError::new(
                "INVALID_RANGE",
                format!("{name} must be a non-negative integer, got {n}"),
            )
            // Numeric like every other `details.rev` (`null` for NaN/±∞, which JSON lacks).
            .with_details(serde_json::Value::Object(
                [(name.to_string(), json!(n))].into_iter().collect(),
            )),
        ))
    }
}

/// `tx.get`: the `CommittedTx`, `{"setAside": {txId, branch, reason, origin}}`, or `null`.
fn tx_json(tx_id: &str, status: TxStatus) -> Result<Reply> {
    Ok(match status {
        TxStatus::Committed(c) => Reply::Json(encode(&c)?),
        TxStatus::SetAside {
            branch,
            reason,
            origin,
        } => Reply::Json(encode(&fin_store::TxSetAside {
            set_aside: fin_store::SetAside {
                tx_id: tx_id.to_string(),
                branch,
                reason,
                origin,
            },
        })?),
        TxStatus::Unknown => Reply::Null,
    })
}

/// `status()`: the store's status with `busy: null`; during a long call, at once, the status
/// cached after the last tick or long call with `busy: {operation, sinceMs}`.
fn status(store: &Shared) -> Result<Reply> {
    let mut guard = lock(&store.state);
    let (value, busy) = match &mut *guard {
        State::Open(s) => {
            let v = serde_json::to_value(s.status()).map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("cannot encode result: {e}"),
                ))
            })?;
            *lock(&store.status) = Some(v.clone());
            (v, serde_json::Value::Null)
        }
        State::Busy {
            operation,
            since_ms,
        } => {
            let Some(v) = lock(&store.status).clone() else {
                return Err(reject(guard.unavailable()));
            };
            (v, json!({ "operation": operation, "sinceMs": since_ms }))
        }
        other => return Err(reject(other.unavailable())),
    };
    drop(guard);
    // The font summary (plan 4C Decision 34): `fonts`, and its warnings after the store's.
    let mut value = store.fonts.merged(value);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("busy".to_string(), busy);
    }
    Ok(Reply::Json(encode(&value)?))
}

/// `setCollect`: the flag in the project file (the lease's), then in the journal at once
/// (the `collect` event the store queued follows with the next tick).
fn set_collect(store: &Shared, collect: &str) -> Result<Reply> {
    let policy: fin_store::CollectPolicy =
        serde_json::from_value(json!(collect)).map_err(|_| {
            reject(
                CoreError::new(
                    "INVALID_ARGS",
                    format!("collect is default or never, not {collect:?}"),
                )
                .with_details(json!({ "collect": collect })),
            )
        })?;
    with_store(store, |s| {
        s.set_collect(policy).map_err(reject)?;
        if let Some(g) = lock(&store.journal).as_ref() {
            g.update(policy);
        }
        Ok(Reply::Json(encode(
            &json!({ "project": s.project_id(), "collect": policy }),
        )?))
    })
}

fn run(store: &Shared, call: Call) -> Result<Reply> {
    match call {
        Call::Head { branch } => with_store(store, |s| Ok(Reply::Number(s.head(&branch)))),
        Call::Commit {
            tx_json,
            generation_json,
            meta_json,
        } => {
            let tx = parse_tx(&tx_json)?;
            let generation = generation_json
                .as_deref()
                .map(parse_generation)
                .transpose()?;
            with_store(store, |s| {
                // Logged as `log.commit` from the typed result (`commit_log`).
                let json = crate::commit_log::commit(
                    s,
                    &store.diag,
                    tx,
                    generation.as_ref(),
                    meta_json.as_deref(),
                    // Decision 34: a commit that changed the pins fetches what they lack.
                    |s, c| store.fonts.committed(s, c),
                )
                .map_err(reject)?;
                Ok(Reply::Json(json))
            })
        }
        Call::Snapshot { branch } => with_store(store, |s| {
            let (rev, project) = s.snapshot(&branch).map_err(reject)?;
            let generation = s.generation_token(&branch);
            Ok(Reply::Json(encode(
                &json!({ "rev": rev, "project": project, "generation": generation }),
            )?))
        }),
        Call::OpsSince { branch, after } => {
            let after = parse_count("rev", after)?;
            with_store(store, |s| {
                Ok(Reply::Json(encode(
                    &s.ops_since(&branch, after).map_err(reject)?,
                )?))
            })
        }
        Call::OpsSinceAt { branch, after } => {
            let after = parse_count("rev", after)?;
            with_store(store, |s| {
                let (generation, ops) = s.ops_since_at(&branch, after).map_err(reject)?;
                Ok(Reply::Json(encode(
                    &json!({ "generation": generation, "ops": ops }),
                )?))
            })
        }
        Call::Status => status(store),
        Call::TxGet { tx_id } => with_store(store, |s| {
            tx_json(&tx_id, s.tx_status(&tx_id).map_err(reject)?)
        }),
        Call::AcknowledgeRewind { branch, generation } => {
            let generation = parse_count("generation", generation)?;
            with_store(store, |s| {
                Ok(Reply::Bool(
                    s.acknowledge_rewind(&branch, generation).map_err(reject)?,
                ))
            })
        }
        Call::AcknowledgeSalvage { copy } => with_store(store, |s| {
            Ok(Reply::Bool(s.acknowledge_salvage(&copy).map_err(reject)?))
        }),
        call @ (Call::BranchCreate { .. }
        | Call::Branches { .. }
        | Call::BranchSet { .. }
        | Call::ResolveBranch { .. }
        | Call::Undo { .. }
        | Call::Redo { .. }
        | Call::Checkpoint { .. }
        | Call::Merge { .. }
        | Call::CherryPick { .. }
        | Call::Revert { .. }) => crate::history::run(store, call),
        call @ (Call::EvalFrame { .. } | Call::EvalAudio { .. } | Call::FontsPlan { .. }) => {
            crate::eval::run(store, call)
        }
        Call::SetCollect { collect } => set_collect(store, &collect),
        #[cfg(feature = "test-hooks")]
        Call::Panic => with_store(store, |_| panic!("fin-node test panic")),
    }
}

impl Call {
    /// The RPC method this call serves (`error.internal.call`; `other` for none).
    fn rpc(&self) -> &'static str {
        match self {
            Call::Commit { .. } => "tx.commit",
            Call::Snapshot { .. } => "snapshot.get",
            Call::OpsSince { .. } | Call::OpsSinceAt { .. } => "ops.subscribe",
            Call::Status => "project.status",
            Call::TxGet { .. } => "tx.get",
            Call::AcknowledgeRewind { .. } => "project.acknowledgeRewind",
            Call::AcknowledgeSalvage { .. } => "project.acknowledgeSalvage",
            Call::BranchCreate { .. } => "branch.create",
            Call::Branches { .. } | Call::ResolveBranch { .. } => "branch.list",
            Call::BranchSet { .. } => "branch.set",
            Call::Undo { .. } => "history.undo",
            Call::Redo { .. } => "history.redo",
            Call::Checkpoint { .. } => "history.checkpoint",
            Call::Merge { .. } => "history.merge",
            Call::CherryPick { .. } => "history.cherryPick",
            Call::Revert { .. } => "history.revert",
            Call::EvalFrame { .. } => "eval.frame",
            Call::EvalAudio { .. } => "eval.audio",
            Call::FontsPlan { .. } => "fonts.plan",
            Call::SetCollect { .. } => "diagnostics.project",
            _ => "other",
        }
    }
}

impl Task for StoreTask {
    type Output = Reply;
    type JsValue = Either4<f64, String, bool, Null>;

    fn compute(&mut self) -> Result<Reply> {
        let Some(call) = self.call.take() else {
            return Err(reject(CoreError::new("INTERNAL", "task ran twice")));
        };
        let store = &self.store;
        let rpc = call.rpc();
        let out = guarded(
            || run(store, call),
            || {
                // Only an open store is given up: the panic happened while using it.
                let mut g = lock(&store.state);
                if matches!(*g, State::Open(_)) {
                    *g = State::Panicked;
                }
            },
        );
        if let Err(e) = &out
            && store.diag.gate().is_some()
        {
            let e = crate::error_value(e);
            crate::commit_log::internal(&store.diag, Some(&store.project), rpc, &e);
        }
        out
    }

    fn resolve(&mut self, _env: Env, reply: Reply) -> Result<Self::JsValue> {
        Ok(match reply {
            Reply::Number(n) => Either4::A(n as f64),
            Reply::Json(s) => Either4::B(s),
            Reply::Bool(b) => Either4::C(b),
            Reply::Null => Either4::D(Null),
        })
    }
}
