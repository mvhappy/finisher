//! Edit intents (plan D, Decision 22; content `txIntent`): a [`Tx`] as the journal keeps it.
//! Content is still "never raw" (Decision 4): [`IntentTx::from_tx`] keys the free-text author,
//! a txId that is not a ULID (an idempotency key), the unkeyed `argsHash` and every
//! `asset.add` path (the one op field that holds a file path; the other `path`s of ops are
//! parameter paths), and keeps the ops otherwise as they are (captions, names and labels are
//! the user's content). Pure: the journal passes its key.
//!
//! The intent's serialised size is computed once, exactly, when it is built (a counting
//! writer, no allocation), for the journal's byte bound and the 32 KiB rule of
//! [`super::payloads::LogCommit::attach_intent`].

use std::fmt;

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::approx::ApproxBytes;
use super::class::{BranchId, Hashed, IdRef, PathRef, PathRoot};
use crate::ops::Op;
use crate::tx::Tx;

/// The most transactions an intent holds (a history call's records).
pub const MAX_INTENT_TXS: usize = 64;

/// An entry with an intent is at most this large; past it the intent is left out
/// (`intentOmitted: tooLarge`) and the builder takes it from the op log.
pub const MAX_INTENT_ENTRY_BYTES: usize = 32 * 1024;

/// One transaction of an intent. Built only by [`IntentTx::from_tx`] (or decoded, which
/// refuses a raw `asset.add` path); its fields are read through accessors.
#[derive(Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IntentTx {
    tx_id: IdRef,
    author: Hashed,
    // Absent when the tx named its branch by name (a request that failed before the store
    // resolved it); the entry's `ctx.branch` says which branch it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BranchId")]
    branch: Option<BranchId>,
    base_rev: u64,
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    args_hash: Option<Hashed>,
    /// The ops, each `asset.add` path a keyed path token ([`PathRef::token`]).
    ops: Vec<Op>,
    #[serde(skip)]
    bytes: usize,
}

impl IntentTx {
    /// The intent of `tx`, consumed (nothing is cloned). `key` is the journal's keyed hash
    /// (16 bytes): the author, the `argsHash`, a txId that is not a canonical ULID and each
    /// `asset.add` path are keyed. A branch that is not a branch id (a name) is left out.
    pub fn from_tx(tx: Tx, key: &dyn Fn(&[u8]) -> [u8; 16]) -> IntentTx {
        let Tx {
            tx_id,
            author,
            branch,
            base_rev,
            schema_version,
            mut ops,
            args_hash,
        } = tx;
        for op in &mut ops {
            if let Op::AssetAdd(a) = op {
                a.path = PathRef::of_path(&a.path, key).token();
            }
        }
        let mut out = IntentTx {
            tx_id: IdRef::keyed(&tx_id, key),
            author: Hashed::keyed(author.as_bytes(), key),
            branch: BranchId::parse(&branch),
            base_rev,
            schema_version,
            args_hash: args_hash.map(|h| Hashed::keyed(h.as_bytes(), key)),
            ops,
            bytes: 0,
        };
        out.bytes = serialised_len(&out);
        out
    }

    pub fn tx_id(&self) -> &IdRef {
        &self.tx_id
    }

    pub fn author(&self) -> &Hashed {
        &self.author
    }

    pub fn branch(&self) -> Option<&BranchId> {
        self.branch.as_ref()
    }

    pub fn base_rev(&self) -> u64 {
        self.base_rev
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn args_hash(&self) -> Option<&Hashed> {
        self.args_hash.as_ref()
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// The exact serialised size, in bytes.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl fmt::Debug for IntentTx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntentTx")
            .field("tx_id", &self.tx_id)
            .field("author", &self.author)
            .field("branch", &self.branch)
            .field("base_rev", &self.base_rev)
            .field("schema_version", &self.schema_version)
            .field("args_hash", &self.args_hash)
            .field("ops", &self.ops)
            .finish()
    }
}

// The cached size is not part of the value.
impl PartialEq for IntentTx {
    fn eq(&self, other: &IntentTx) -> bool {
        self.tx_id == other.tx_id
            && self.author == other.author
            && self.branch == other.branch
            && self.base_rev == other.base_rev
            && self.schema_version == other.schema_version
            && self.args_hash == other.args_hash
            && self.ops == other.ops
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Decoded {
    tx_id: IdRef,
    author: Hashed,
    #[serde(default)]
    branch: Option<BranchId>,
    base_rev: u64,
    schema_version: u32,
    #[serde(default)]
    args_hash: Option<Hashed>,
    ops: Vec<Op>,
}

impl<'de> Deserialize<'de> for IntentTx {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let x = Decoded::deserialize(d)?;
        let raw_path = x
            .ops
            .iter()
            .any(|op| matches!(op, Op::AssetAdd(a) if PathRef::from_token(&a.path).is_none()));
        if raw_path {
            return Err(D::Error::custom(
                "an intent's asset.add path is not a path token",
            ));
        }
        let mut out = IntentTx {
            tx_id: x.tx_id,
            author: x.author,
            branch: x.branch,
            base_rev: x.base_rev,
            schema_version: x.schema_version,
            args_hash: x.args_hash,
            ops: x.ops,
            bytes: 0,
        };
        out.bytes = serialised_len(&out);
        Ok(out)
    }
}

/// An edit intent (content `txIntent`): the transactions as committed (an edit's one tx, or a
/// history call's records, at most [`MAX_INTENT_TXS`]), or the request's normalised tx on
/// failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TxIntent(pub Vec<IntentTx>);

impl TxIntent {
    /// The intent of `txs` (at most the first [`MAX_INTENT_TXS`]), each by
    /// [`IntentTx::from_tx`].
    pub fn from_txs(txs: impl IntoIterator<Item = Tx>, key: &dyn Fn(&[u8]) -> [u8; 16]) -> Self {
        TxIntent(
            txs.into_iter()
                .take(MAX_INTENT_TXS)
                .map(|tx| IntentTx::from_tx(tx, key))
                .collect(),
        )
    }

    /// The exact serialised size, in bytes (`[`, the txs and their commas, `]`).
    pub fn bytes(&self) -> usize {
        2 + self.0.iter().map(IntentTx::bytes).sum::<usize>() + self.0.len().saturating_sub(1)
    }
}

impl ApproxBytes for TxIntent {
    fn approx_bytes(&self) -> usize {
        self.bytes()
    }
}

impl PathRef {
    /// The [`PathRef`] of a file path: the keyed hash of the path, its media extension, and
    /// its root (`project` for a relative path; `other` for an absolute one, whose root only
    /// the emitter could tell).
    pub fn of_path(path: &str, key: &dyn Fn(&[u8]) -> [u8; 16]) -> PathRef {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e);
        let absolute = path.starts_with(['/', '\\', '~'])
            || path.as_bytes().get(1) == Some(&b':')
            || path.is_empty();
        let root = if absolute {
            PathRoot::Other
        } else {
            PathRoot::Project
        };
        PathRef::new(root, ext, Hashed::keyed(path.as_bytes(), key))
    }

    /// The path's text form, where an op must hold a string: `<root>/<hash>` and `.<ext>` when
    /// it has a media extension (`<other>/k1:<32 hex>.mp4`).
    pub fn token(&self) -> String {
        let ext = self.ext.map(|e| format!(".{e}")).unwrap_or_default();
        format!("<{}>/{}{ext}", root_name(self.root), self.hash)
    }

    /// [`PathRef::token`] read back; `None` for anything else (a raw path).
    pub fn from_token(s: &str) -> Option<PathRef> {
        let (root, rest) = s.strip_prefix('<')?.split_once(">/")?;
        let root = ROOTS.iter().find(|r| root_name(**r) == root)?;
        let (hash, ext) = match rest.split_once('.') {
            Some((h, e)) => (h, Some(e)),
            None => (rest, None),
        };
        let path = PathRef::new(*root, ext, Hashed::parse(hash)?);
        (ext.is_none() || path.ext.is_some()).then_some(path)
    }
}

const ROOTS: [PathRoot; 5] = [
    PathRoot::Project,
    PathRoot::Library,
    PathRoot::Workspace,
    PathRoot::ServerData,
    PathRoot::Other,
];

fn root_name(root: PathRoot) -> &'static str {
    match root {
        PathRoot::Project => "project",
        PathRoot::Library => "library",
        PathRoot::Workspace => "workspace",
        PathRoot::ServerData => "serverData",
        PathRoot::Other => "other",
    }
}

/// Counts the bytes written to it (it does no I/O).
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The length of `value`'s compact JSON, without building it.
pub(crate) fn serialised_len<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut c = Counter(0);
    serde_json::to_writer(&mut c, value).map_or(0, |()| c.0)
}
