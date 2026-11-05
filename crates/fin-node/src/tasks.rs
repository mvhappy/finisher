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

fn parse_tx(json: &str) -> Result<Tx> {
    serde_json::from_str(json).map_err(|e| {
        reject(
            CoreError::new("INVALID_OP", format!("transaction does not decode: {e}"))
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix("send a transaction matching the fin-core Tx schema"),
        )
    })
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
            .with_details(serde_json::Value::Object(
                [(name.to_string(), n.to_string().into())]
                    .into_iter()
                    .collect(),
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
        } => Reply::Json(encode(&json!({ "setAside": {
            "txId": tx_id, "branch": branch, "reason": reason, "origin": origin,
        }}))?),
        TxStatus::Unknown => Reply::Null,
    })
}

fn run(store: &Shared, call: Call) -> Result<Reply> {
    match call {
        Call::Head { branch } => with_store(store, |s| Ok(Reply::Number(s.head(&branch)))),
        Call::Commit {
            tx_json,
            generation_json,
        } => {
            let tx = parse_tx(&tx_json)?;
            let generation = generation_json
                .as_deref()
                .map(parse_generation)
                .transpose()?;
            with_store(store, |s| {
                let c = s
                    .commit_at_generation(tx, generation.as_ref())
                    .map_err(reject)?;
                Ok(Reply::Json(encode(&c)?))
            })
        }
        Call::Snapshot { branch } => with_store(store, |s| {
            let (rev, project) = s.snapshot(&branch).map_err(reject)?;
            Ok(Reply::Json(encode(
                &json!({ "rev": rev, "project": project }),
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
        Call::Status => with_store(store, |s| Ok(Reply::Json(encode(&s.status())?))),
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
        #[cfg(feature = "test-hooks")]
        Call::Panic => with_store(store, |_| panic!("fin-node test panic")),
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
        guarded(
            || run(store, call),
            || *lock(&store.state) = State::Panicked,
        )
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
