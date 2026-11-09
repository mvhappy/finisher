//! Branch and history calls (plan 4B Task 17): a second `#[napi] impl Engine` block.
//!
//! Requests are the camelCase JSON of the `fin-store` request structs (they derive
//! `Deserialize`); results are the wire types' JSON. Everything but `recover` is a short call
//! ([`crate::tasks`]); `recover` may import a folder and copy many records, so it is a long call
//! on the worker. A dry run plans against the local database and keeps nothing, on a follower
//! too.
//!
//! Every napi method that writes takes an optional `metaJson` (`{actor?, guard?}`): `actor`
//! (finisherd's `callMeta`) is the actor of its `log.commit` entry ([`crate::commit_log`]);
//! `guard` (a `WriteGuard`, `{denyBranches?, allowBranches?}` of branch ids, from the
//! chain's policy stage) is checked by the store inside the write transaction against the
//! branch the call changes (`BRANCH_PROTECTED`, `BRANCH_NOT_ALLOWED`). A `metaJson` or
//! `guard` that does not decode is `INVALID_OP`; it is never ignored.

use fin_core::CoreError;
use fin_core::events::payloads::CommitRecord;
use fin_journal::Journal;
use fin_store::{
    BranchCreate, BranchSet, CheckpointReq, CherryPickReq, DropReq, HistoryReq, MergeReq,
    RecoverReq, RevertReq, Store,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::commit_log::{self, Req};
use crate::engine::{Engine, Shared, with_store};
use crate::tasks::{Call, Reply, StoreTask};
use crate::{encode, reject};

/// Decodes a request; a bad one is `INVALID_OP` naming `what`.
fn parse<T: DeserializeOwned>(what: &str, json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|e| {
        reject(
            CoreError::new("INVALID_OP", format!("{what} request does not decode: {e}"))
                .with_details(json!({ "reason": e.to_string() }))
                .with_fix(format!(
                    "send the camelCase {what} request (see @finisher/contracts)"
                )),
        )
    })
}

/// The request side of a `log.commit` entry, taken only while the journal is on.
fn req(j: &Journal, tx_id: &str, author: &str, branch: Option<&str>) -> Option<Req> {
    j.gate().map(|_| Req {
        tx_id: tx_id.to_owned(),
        author: author.to_owned(),
        branch: branch.map(str::to_owned),
    })
}

/// The short history calls (called from [`crate::tasks`]). Every one that writes is logged
/// as `log.commit` ([`commit_log::history`]); the listings are not.
pub(crate) fn run(store: &Shared, call: Call) -> Result<Reply> {
    with_store(store, |s| {
        let j = &store.diag;
        let json = match call {
            Call::Branches { all } => {
                return Ok(Reply::Json(encode(&s.branches(all).map_err(reject)?)?));
            }
            Call::ResolveBranch { name } => {
                return Ok(Reply::Json(encode(
                    &s.resolve_branch(&name).map_err(reject)?,
                )?));
            }
            Call::BranchCreate {
                req_json,
                meta_json,
            } => {
                let r = parse::<BranchCreate>("branch create", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.from));
                commit_log::history(s, j, CommitRecord::Branch, meta_json.as_deref(), q, |s| {
                    s.create_branch(r)
                })
            }
            Call::BranchSet {
                req_json,
                meta_json,
            } => {
                let r = parse::<BranchSet>("branch set", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.branch));
                commit_log::history(
                    s,
                    j,
                    CommitRecord::BranchSet,
                    meta_json.as_deref(),
                    q,
                    |s| s.set_branch(r),
                )
            }
            Call::Undo {
                req_json,
                meta_json,
            } => {
                let r = parse::<HistoryReq>("undo", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.branch));
                commit_log::history(s, j, CommitRecord::Undo, meta_json.as_deref(), q, |s| {
                    s.undo(r)
                })
            }
            Call::Redo {
                req_json,
                meta_json,
            } => {
                let r = parse::<HistoryReq>("redo", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.branch));
                commit_log::history(s, j, CommitRecord::Redo, meta_json.as_deref(), q, |s| {
                    s.redo(r)
                })
            }
            Call::Checkpoint {
                req_json,
                meta_json,
            } => {
                let r = parse::<CheckpointReq>("checkpoint", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.branch));
                commit_log::history(
                    s,
                    j,
                    CommitRecord::Checkpoint,
                    meta_json.as_deref(),
                    q,
                    |s| s.checkpoint(r),
                )
            }
            Call::Merge {
                req_json,
                meta_json,
            } => {
                let r = parse::<MergeReq>("merge", &req_json)?;
                // The group's first record, `<id>-1`, on every path (what `fin` reports).
                let q = req(j, &format!("{}-1", r.tx_id), &r.author, Some(&r.into));
                commit_log::history(s, j, CommitRecord::Merge, meta_json.as_deref(), q, |s| {
                    s.merge(r)
                })
            }
            Call::CherryPick {
                req_json,
                meta_json,
            } => {
                let r = parse::<CherryPickReq>("cherry-pick", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.into));
                commit_log::history(s, j, CommitRecord::Pick, meta_json.as_deref(), q, |s| {
                    s.cherry_pick(r)
                })
            }
            Call::Revert {
                req_json,
                meta_json,
            } => {
                let r = parse::<RevertReq>("revert", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.branch));
                commit_log::history(s, j, CommitRecord::Revert, meta_json.as_deref(), q, |s| {
                    s.revert(r)
                })
            }
            Call::Drop {
                req_json,
                meta_json,
            } => {
                let r = parse::<DropReq>("drop", &req_json)?;
                let q = req(j, &r.tx_id, &r.author, Some(&r.into));
                commit_log::history(s, j, CommitRecord::Drop, meta_json.as_deref(), q, |s| {
                    s.drop(r)
                })
            }
            _ => unreachable!("not a history call"),
        }
        .map_err(reject)?;
        // A branch or history write may change any branch's pins (Decision 34).
        store.fonts.rescan_all(s);
        Ok(Reply::Json(json))
    })
}

/// `recover`: a listing is not logged; a request that makes or dismisses something is a
/// `log.commit` of record `recover`.
fn recover(s: &mut Store, j: &Journal, req_json: &str, meta: Option<&str>) -> Result<String> {
    let r = parse::<RecoverReq>("recover", req_json)?;
    if r.create.is_none() && r.dismiss.is_none() {
        // A listing, an import or a dry run with them: under the guard too (a dry run plans
        // through `Store::write`).
        let guard = crate::journal::ctx::guard_of_meta(meta).map_err(reject)?;
        return encode(&s.with_guard(guard, |s| s.recover(r)).map_err(reject)?);
    }
    // The recovery's first record, `<id>-1`, on every path (what `fin` reports).
    let q = req(j, &format!("{}-1", r.tx_id), &r.author, None);
    commit_log::history(s, j, CommitRecord::Recover, meta, q, |s| s.recover(r)).map_err(reject)
}

#[napi]
impl Engine {
    /// Makes a branch (`BranchCreate` JSON); resolves to the `BranchCreated` JSON.
    #[napi(catch_unwind)]
    pub fn branch_create(
        &self,
        req_json: String,
        meta_json: Option<String>,
    ) -> AsyncTask<StoreTask> {
        self.task(Call::BranchCreate {
            req_json,
            meta_json,
        })
    }

    /// Resolves to the `BranchView[]` JSON (`main` first; with `all`, archived ones too).
    #[napi(catch_unwind)]
    pub fn branches(&self, all: bool) -> AsyncTask<StoreTask> {
        self.task(Call::Branches { all })
    }

    /// Renames, archives or restores a branch (`BranchSet` JSON); resolves to the
    /// `CommittedTx` JSON.
    #[napi(catch_unwind)]
    pub fn branch_set(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::BranchSet {
            req_json,
            meta_json,
        })
    }

    /// Resolves to the JSON string of the branch id `name` (an id, a name, or `<name>#<id>`)
    /// names; `BRANCH_NOT_FOUND` otherwise.
    #[napi(catch_unwind)]
    pub fn resolve_branch(&self, name: String) -> AsyncTask<StoreTask> {
        self.task(Call::ResolveBranch { name })
    }

    /// Undoes (`HistoryReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn undo(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Undo {
            req_json,
            meta_json,
        })
    }

    /// Redoes (`HistoryReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn redo(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Redo {
            req_json,
            meta_json,
        })
    }

    /// Names the head of a branch (`CheckpointReq` JSON); resolves to the `CommittedTx` JSON.
    #[napi(catch_unwind)]
    pub fn checkpoint(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Checkpoint {
            req_json,
            meta_json,
        })
    }

    /// Merges a branch into another (`MergeReq` JSON); resolves to the `MergeReport` JSON.
    #[napi(catch_unwind)]
    pub fn merge(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Merge {
            req_json,
            meta_json,
        })
    }

    /// Copies one record onto a branch (`CherryPickReq` JSON); resolves to the `HistoryResult`
    /// JSON.
    #[napi(catch_unwind)]
    pub fn cherry_pick(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::CherryPick {
            req_json,
            meta_json,
        })
    }

    /// Reverts a merge group (`RevertReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn revert(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Revert {
            req_json,
            meta_json,
        })
    }

    /// Drops (and undrops) origins of a source branch for a target (`DropReq` JSON); resolves
    /// to the `CommittedTx` JSON of the `drop` record.
    #[napi(catch_unwind)]
    pub fn drop(&self, req_json: String, meta_json: Option<String>) -> AsyncTask<StoreTask> {
        self.task(Call::Drop {
            req_json,
            meta_json,
        })
    }

    /// Lists the recoverable groups, and with `create` makes their branches (`RecoverReq`
    /// JSON); resolves to the `RecoverReport` JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn recover<'e>(
        &self,
        env: &'e Env,
        req_json: String,
        meta_json: Option<String>,
    ) -> Result<Object<'e>> {
        let j = self.inner.diag.clone();
        self.long(env, "recover", move |s| {
            recover(s, &j, &req_json, meta_json.as_deref())
        })
    }
}
