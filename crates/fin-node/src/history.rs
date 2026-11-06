//! Branch and history calls (plan 4B Task 17): a second `#[napi] impl Engine` block.
//!
//! Requests are the camelCase JSON of the `fin-store` request structs (they derive
//! `Deserialize`); results are the wire types' JSON. Everything but `recover` is a short call
//! ([`crate::tasks`]); `recover` may import a folder and copy many records, so it is a long call
//! on the worker. A dry run plans against the local database and keeps nothing, on a follower
//! too.

use fin_core::CoreError;
use fin_store::{
    BranchCreate, BranchSet, CheckpointReq, CherryPickReq, HistoryReq, MergeReq, RecoverReq,
    RevertReq, Store,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::de::DeserializeOwned;
use serde_json::json;

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

/// The short history calls (called from [`crate::tasks`]).
pub(crate) fn run(store: &Shared, call: Call) -> Result<Reply> {
    with_store(store, |s| {
        let json = match call {
            Call::BranchCreate { req_json } => encode(
                &s.create_branch(parse::<BranchCreate>("branch create", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::Branches { all } => encode(&s.branches(all).map_err(reject)?)?,
            Call::BranchSet { req_json } => encode(
                &s.set_branch(parse::<BranchSet>("branch set", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::ResolveBranch { name } => encode(&s.resolve_branch(&name).map_err(reject)?)?,
            Call::Undo { req_json } => encode(
                &s.undo(parse::<HistoryReq>("undo", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::Redo { req_json } => encode(
                &s.redo(parse::<HistoryReq>("redo", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::Checkpoint { req_json } => encode(
                &s.checkpoint(parse::<CheckpointReq>("checkpoint", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::Merge { req_json } => encode(
                &s.merge(parse::<MergeReq>("merge", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::CherryPick { req_json } => encode(
                &s.cherry_pick(parse::<CherryPickReq>("cherry-pick", &req_json)?)
                    .map_err(reject)?,
            )?,
            Call::Revert { req_json } => encode(
                &s.revert(parse::<RevertReq>("revert", &req_json)?)
                    .map_err(reject)?,
            )?,
            _ => unreachable!("not a history call"),
        };
        Ok(Reply::Json(json))
    })
}

fn recover(s: &mut Store, req_json: &str) -> Result<String> {
    encode(
        &s.recover(parse::<RecoverReq>("recover", req_json)?)
            .map_err(reject)?,
    )
}

#[napi]
impl Engine {
    /// Makes a branch (`BranchCreate` JSON); resolves to the `BranchCreated` JSON.
    #[napi(catch_unwind)]
    pub fn branch_create(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::BranchCreate { req_json })
    }

    /// Resolves to the `BranchView[]` JSON (`main` first; with `all`, archived ones too).
    #[napi(catch_unwind)]
    pub fn branches(&self, all: bool) -> AsyncTask<StoreTask> {
        self.task(Call::Branches { all })
    }

    /// Renames, archives or restores a branch (`BranchSet` JSON); resolves to the
    /// `CommittedTx` JSON.
    #[napi(catch_unwind)]
    pub fn branch_set(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::BranchSet { req_json })
    }

    /// Resolves to the JSON string of the branch id `name` (an id, a name, or `<name>#<id>`)
    /// names; `BRANCH_NOT_FOUND` otherwise.
    #[napi(catch_unwind)]
    pub fn resolve_branch(&self, name: String) -> AsyncTask<StoreTask> {
        self.task(Call::ResolveBranch { name })
    }

    /// Undoes (`HistoryReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn undo(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Undo { req_json })
    }

    /// Redoes (`HistoryReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn redo(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Redo { req_json })
    }

    /// Names the head of a branch (`CheckpointReq` JSON); resolves to the `CommittedTx` JSON.
    #[napi(catch_unwind)]
    pub fn checkpoint(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Checkpoint { req_json })
    }

    /// Merges a branch into another (`MergeReq` JSON); resolves to the `MergeReport` JSON.
    #[napi(catch_unwind)]
    pub fn merge(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Merge { req_json })
    }

    /// Copies one record onto a branch (`CherryPickReq` JSON); resolves to the `HistoryResult`
    /// JSON.
    #[napi(catch_unwind)]
    pub fn cherry_pick(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::CherryPick { req_json })
    }

    /// Reverts a merge group (`RevertReq` JSON); resolves to the `HistoryResult` JSON.
    #[napi(catch_unwind)]
    pub fn revert(&self, req_json: String) -> AsyncTask<StoreTask> {
        self.task(Call::Revert { req_json })
    }

    /// Lists the recoverable groups, and with `create` makes their branches (`RecoverReq`
    /// JSON); resolves to the `RecoverReport` JSON (long call).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn recover<'e>(&self, env: &'e Env, req_json: String) -> Result<Object<'e>> {
        self.long(env, "recover", move |s| recover(s, &req_json))
    }
}
