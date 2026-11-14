//! Migration 0001: the sub-project 2 layout → hashed commit records (sub-project 4 spec §3.1).
//!
//! **Frozen at schema v1.** Nothing here uses the current `fin-core` types, so later model
//! changes cannot alter what this step writes (`migration_0001_output_is_pinned` pins it):
//!
//! - Each old tx becomes a v1 record line, built by [`RecordV1`] in record field order:
//!   `seq` = its `rev`; `intent`, `inverse` and `bindings` are its stored JSON pasted verbatim
//!   ([`RawValue`]); `resolved` is `intent.ops` with `$placeholders` replaced in the v1
//!   reference fields ([`REFS_V1`]), keeping each op's field order; `txId`, `author` and
//!   `schemaVersion` come from the intent; `committedAt` is the stored one; `kind` is `edit`.
//!   Today these are byte-identical to what `fin_core::encode_record` writes for the same tx
//!   (`migration_0001_matches_encode_record`). Later code reads them through `upcast_record`.
//! - The bytes are hashed and chained per branch in rev order (`parent_hash` null for the
//!   first), at epoch 0 and not exported.
//! - `txs` and `snapshots` are rebuilt (old table renamed aside, v1 table created, filled, old
//!   dropped): `txs` gets the v1 columns, `snapshots.rev` becomes `seq`. Snapshot JSON is kept
//!   as it is; it loads through `fin_core::upcast_snapshot`.
//! - The tables 4A adds (`segments`, `lease_state`, `quarantine`) and the `txs_unexported`
//!   index are created, the header gets `application_id`, and `meta` gets `formatVersion` and
//!   `minReaderVersion`.

use std::collections::{BTreeMap, HashMap};

use fin_core::CoreError;
use rusqlite::{Transaction, params};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::RawValue;

use crate::StoreError;
use crate::db::{hash_bytes, set_meta};
use crate::migrations::APPLICATION_ID;

const V1_TABLES: &str = "
ALTER TABLE txs RENAME TO txs_sp2;
CREATE TABLE txs (
    seq INTEGER NOT NULL,
    branch TEXT NOT NULL,
    tx_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    tx_hash TEXT NOT NULL,
    parent_hash TEXT,
    epoch INTEGER NOT NULL,
    exported INTEGER NOT NULL,
    diff TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    committed_at INTEGER NOT NULL,
    PRIMARY KEY (branch, seq)
);
ALTER TABLE snapshots RENAME TO snapshots_sp2;
CREATE TABLE snapshots (
    branch TEXT NOT NULL,
    seq INTEGER NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (branch, seq)
);
INSERT INTO snapshots (branch, seq, json) SELECT branch, rev, json FROM snapshots_sp2;
DROP TABLE snapshots_sp2;
CREATE TABLE segments (
    branch TEXT NOT NULL,
    hash TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    first_seq INTEGER NOT NULL,
    last_seq INTEGER NOT NULL,
    pack INTEGER NOT NULL,
    replaces TEXT NOT NULL,
    state TEXT NOT NULL
);
CREATE TABLE lease_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE quarantine (
    id INTEGER PRIMARY KEY,
    tx_hash TEXT NOT NULL UNIQUE,
    branch TEXT NOT NULL,
    origin TEXT NOT NULL,
    reason TEXT NOT NULL,
    detail TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    added_at INTEGER NOT NULL,
    seen_at INTEGER NOT NULL,
    current INTEGER NOT NULL
);
";

/// The v1 reference fields of each v1 op: top-level fields, then fields of the nested `clip`
/// object. Only these may hold `$placeholders`.
const REFS_V1: &[(&str, &[&str], &[&str])] = &[
    ("asset.add", &["id"], &[]),
    ("asset.remove", &["id"], &[]),
    ("clip.insert", &["id", "track", "asset"], &[]),
    ("clip.trim", &["clip"], &[]),
    ("clip.delete", &["clip"], &[]),
    ("clip.remove", &["id"], &[]),
    ("clip.restore", &["track"], &["id", "asset"]),
];

/// A v1 commit record line, fields in record order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordV1<'a> {
    tx_id: &'a str,
    seq: u64,
    kind: &'static str,
    schema_version: u64,
    parent_hash: Option<&'a str>,
    author: &'a str,
    committed_at: i64,
    intent: &'a RawValue,
    resolved: Vec<Obj>,
    inverse: &'a RawValue,
    bindings: &'a RawValue,
}

/// The fields of a stored SP2 intent this step needs.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntentV1 {
    tx_id: String,
    author: String,
    schema_version: u64,
    ops: Vec<Box<RawValue>>,
}

/// A JSON object in its written field order, values kept as raw JSON text.
struct Obj(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Obj {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Obj;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Obj, A::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    fields.push(entry);
                }
                Ok(Obj(fields))
            }
        }
        d.deserialize_map(V)
    }
}

impl Serialize for Obj {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl Obj {
    fn field(&mut self, name: &str) -> Option<&mut Box<RawValue>> {
        self.0.iter_mut().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

type OldRow = (i64, String, String, String, String, String, i64);

fn bad(rev: i64, why: impl std::fmt::Display) -> StoreError {
    CoreError::new(
        "INVALID_FORMAT",
        format!("sub-project 2 tx at rev {rev} does not decode: {why}"),
    )
    .with_details(json!({ "kind": "sp2Tx", "rev": rev, "reason": why.to_string() }))
    .into()
}

/// Replaces a `$placeholder` string with its binding (an unbound one is
/// `UNBOUND_PLACEHOLDER`); any other value is kept as written.
fn resolve_ref(
    raw: &mut Box<RawValue>,
    bindings: &BTreeMap<String, String>,
    rev: i64,
    op_index: usize,
) -> Result<(), StoreError> {
    let text: String = serde_json::from_str(raw.get()).map_err(|e| bad(rev, e))?;
    if !text.starts_with('$') {
        return Ok(());
    }
    let Some(id) = bindings.get(&text) else {
        return Err(CoreError::new(
            "UNBOUND_PLACEHOLDER",
            format!("placeholder {text} has no recorded binding"),
        )
        .with_details(json!({ "placeholder": text, "opIndex": op_index, "rev": rev }))
        .into());
    };
    *raw = RawValue::from_string(serde_json::to_string(id)?)?;
    Ok(())
}

/// One v1 op with its reference fields resolved, field order kept.
fn resolve_op(
    op: &RawValue,
    bindings: &BTreeMap<String, String>,
    rev: i64,
    op_index: usize,
) -> Result<Obj, StoreError> {
    let mut obj: Obj = serde_json::from_str(op.get()).map_err(|e| bad(rev, e))?;
    let tag: String = obj
        .field("op")
        .map(|t| serde_json::from_str(t.get()))
        .transpose()
        .map_err(|e| bad(rev, e))?
        .ok_or_else(|| bad(rev, format!("op {op_index} has no \"op\"")))?;
    let (_, top, nested) = REFS_V1
        .iter()
        .find(|(name, _, _)| *name == tag)
        .ok_or_else(|| bad(rev, format!("{tag:?} is not a v1 op")))?;
    for name in *top {
        let raw = obj
            .field(name)
            .ok_or_else(|| bad(rev, format!("{tag} has no {name}")))?;
        resolve_ref(raw, bindings, rev, op_index)?;
    }
    if !nested.is_empty() {
        let raw = obj
            .field("clip")
            .ok_or_else(|| bad(rev, format!("{tag} has no clip")))?;
        let mut clip: Obj = serde_json::from_str(raw.get()).map_err(|e| bad(rev, e))?;
        for name in *nested {
            let inner = clip
                .field(name)
                .ok_or_else(|| bad(rev, format!("{tag} clip has no {name}")))?;
            resolve_ref(inner, bindings, rev, op_index)?;
        }
        *raw = RawValue::from_string(serde_json::to_string(&clip)?)?;
    }
    Ok(obj)
}

/// The v1 record bytes of one SP2 row.
fn record_bytes(
    rev: i64,
    json: &str,
    inverse: &str,
    bindings: &str,
    parent: Option<&str>,
    committed_at: i64,
) -> Result<(String, String, u64, Vec<u8>), StoreError> {
    let intent_raw: &RawValue = serde_json::from_str(json).map_err(|e| bad(rev, e))?;
    let inverse_raw: &RawValue = serde_json::from_str(inverse).map_err(|e| bad(rev, e))?;
    let bindings_raw: &RawValue = serde_json::from_str(bindings).map_err(|e| bad(rev, e))?;
    let intent: IntentV1 = serde_json::from_str(json).map_err(|e| bad(rev, e))?;
    let map: BTreeMap<String, String> = serde_json::from_str(bindings).map_err(|e| bad(rev, e))?;
    let resolved = intent
        .ops
        .iter()
        .enumerate()
        .map(|(i, op)| resolve_op(op, &map, rev, i))
        .collect::<Result<_, _>>()?;
    let record = RecordV1 {
        tx_id: &intent.tx_id,
        seq: u64::try_from(rev).map_err(|_| bad(rev, "negative rev"))?,
        kind: "edit",
        schema_version: intent.schema_version,
        parent_hash: parent,
        author: &intent.author,
        committed_at,
        intent: intent_raw,
        resolved,
        inverse: inverse_raw,
        bindings: bindings_raw,
    };
    let bytes = serde_json::to_vec(&record)?;
    Ok((intent.tx_id, intent.author, intent.schema_version, bytes))
}

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(V1_TABLES)?;
    let mut select = txn.prepare(
        "SELECT rev, branch, json, diff, inverse, bindings, committed_at
         FROM txs_sp2 ORDER BY branch, rev",
    )?;
    let rows = select.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })?;
    let mut insert = txn.prepare(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, ?2, ?3, 'edit', ?4, ?5, ?6, 0, 0, ?7, ?8, ?9, ?10)",
    )?;
    let mut last_hash: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (rev, branch, json, diff, inverse, bindings, committed_at): OldRow = row?;
        let parent = last_hash.get(&branch).cloned();
        let (tx_id, author, schema_version, bytes) = record_bytes(
            rev,
            &json,
            &inverse,
            &bindings,
            parent.as_deref(),
            committed_at,
        )?;
        let hash = hash_bytes(&bytes);
        insert.execute(params![
            rev,
            branch,
            tx_id,
            bytes,
            hash,
            parent,
            diff,
            i64::try_from(schema_version).map_err(|_| bad(rev, "schemaVersion overflows"))?,
            author,
            committed_at,
        ])?;
        last_hash.insert(branch, hash);
    }
    drop(insert);
    drop(select);
    txn.execute_batch(
        "DROP TABLE txs_sp2;
         CREATE INDEX txs_unexported ON txs (branch, seq) WHERE exported = 0;",
    )?;
    txn.pragma_update(None, "application_id", APPLICATION_ID)?;
    set_meta(txn, "formatVersion", "1")?;
    set_meta(txn, "minReaderVersion", "1")?;
    Ok(())
}
