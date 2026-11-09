//! Transaction envelope, committed transactions, diffs and apply results.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::Project;
use crate::ops::Edge;
pub use crate::ops::Op;
use crate::record::Link;
use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tx {
    pub tx_id: String,
    pub author: String,
    pub branch: String,
    pub base_rev: u64,
    pub schema_version: u32,
    pub ops: Vec<Op>,
    /// What produced the tx, for a client that sends an idempotency key as `txId`:
    /// `blake3:<64 hex>` of the command input (op kinds and canonical args, before they were
    /// resolved against a snapshot). A retry rebuilds `ops` against a newer head (positions
    /// move; the stored tx is rebased), so `ops` cannot tell a retry from another command;
    /// this can. The store refuses a `txId` it holds with another `argsHash`
    /// (`IDEM_KEY_REUSED`). Stored with the record, so every server that imported it checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_hash: Option<String>,
    // The plugin command that proposed the tx (spec §3.8). Set only by the server's plugin
    // host (a client's is stripped); ignored by apply and replay; carried by rebase and pick.
    // Absent on every other tx and never written then, so older records keep their bytes.
    // Boxed, as `CommittedTx::link`: most txs have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Via")]
    pub via: Option<Box<Via>>,
}

/// The plugin command a transaction came from: `plugin` is `<id>@<version>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Via {
    pub plugin: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EntityKind {
    Asset,
    Track,
    Timeline,
    // An item of any kind (4B's `clip`, plan 4C Decision 4). Not a doc comment: a described
    // variant splits the generated TypeScript union.
    Item,
    Transition,
    Link,
    Marker,
    Bin,
    // An item's effect, `<item>/<fx>` (plan 4C Decisions 10 and 20).
    Effect,
    // A project caption style, `st<n>` (plan 4C Task 18).
    Style,
    // A font pin, `font/<key>` (plan 4C Decisions 20 and 29).
    Font,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Removed,
    Updated,
}

/// One entity an op changed (plan 4C Decision 4). `op`: the index of the op in its transaction
/// that made the change. The optional fields are omitted on the wire when absent (never
/// `null`):
/// - `track`: for an item, the track it is on (after the op; for a removed item, the track it
///   was taken off), so a footprint and replay's track check see the tracks of ops that record
///   no shift (a lift, a strict or overwrite trim, a roll, a slip, a split); none for assets;
/// - `edge`: the edge an `updated` change moved, if that is all it did to the entity;
/// - `path`: the parameter an `updated` change set (a registry path), if that is all it did;
/// - `by`: the content offset of a start-edge move: every item-relative time of the entity
///   moved by it (plan 4C Decision 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: EntityKind,
    pub id: String,
    pub change: ChangeKind,
    pub op: u32,
    // The optional fields have no doc comment of their own: a schema `$ref` with a sibling
    // `description` would be inlined in the generated TypeScript instead of naming its type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Edge")]
    pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub by: Option<Rational>,
    // A track's timeline, on the change of a `track.move` or `track.remove` (whose ops name
    // only the track): its track order is an entity of its own (`ops::util::lists`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub timeline: Option<String>,
}

/// Items on `track` at or after `from` moved by `by` (ripple), recorded by op `op` of the
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Shift {
    pub track: String,
    pub from: Rational,
    pub by: Rational,
    pub op: u32,
}

/// What a transaction changed, op by op: each entry names the op that recorded it (`op`),
/// and both lists are in op order. An op may record any number of changes and shifts.
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
    /// The record's kind and links; omitted for an `edit`. Boxed: most txs are edits, and a
    /// link would double the size of every `CommittedTx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Box<Link>>,
}
