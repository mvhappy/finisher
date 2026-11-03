//! Transaction envelope, committed transactions, diffs and apply results.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::Project;
use crate::time::Rational;

/// A primitive edit operation. Variants arrive with the op modules (one module per op).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op")]
pub enum Op {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tx {
    pub tx_id: String,
    pub author: String,
    pub branch: String,
    pub base_rev: u64,
    pub schema_version: u32,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EntityKind {
    Asset,
    Track,
    Clip,
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Removed,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: EntityKind,
    pub id: String,
    pub change: ChangeKind,
}

/// Items on `track` at or after `from` moved by `by` (ripple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Shift {
    pub track: String,
    pub from: Rational,
    pub by: Rational,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub changes: Vec<Change>,
    pub shifts: Vec<Shift>,
}

/// The result of applying a transaction to a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub project: Project,
    pub diff: Diff,
    pub inverse: Vec<Op>,
    /// Placeholder (`$name`) to allocated id.
    pub bindings: BTreeMap<String, String>,
}

/// A transaction as persisted: with its revision, resolved bindings and diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommittedTx {
    pub rev: u64,
    pub tx: Tx,
    pub diff: Diff,
    pub bindings: BTreeMap<String, String>,
}
