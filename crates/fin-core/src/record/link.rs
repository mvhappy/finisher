//! Typed record links (plan 4B Decision 1): a record's kind together with its kind fields.
//!
//! On the record the fields are flat and optional ([`Record`]), so `edit` records keep 4A's
//! bytes. [`Record::link`] reads them as one [`Link`] and refuses (`INVALID_FORMAT`) a field set
//! that does not match the kind; [`Record::set_link`] writes exactly the link's fields.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Record, RecordKind, invalid_format};
use crate::error::CoreError;

/// The most origin ids one `drop` record names, `drops` and `undrops` together (plan 5E
/// Decision 1).
pub const MAX_DROP_IDS: usize = 4_096;

/// A record's kind and what it points at (spec §4.2). Serialised (in `CommittedTx.link`) as
/// `{"kind": "<kind>", …fields}`, camelCase, absent optional fields omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Link {
    Edit,
    /// Undoes `undoes` (unit txIds, newest first).
    Undo {
        undoes: Vec<String>,
    },
    /// Reverses the `undo` record `redoes`.
    Redo {
        redoes: String,
    },
    /// A copy of `origin` from branch `from`, in merge group `merge_id`.
    Merge {
        merge_id: String,
        from: String,
        origin: String,
    },
    /// A copy of `origin` (its origin root) from branch `from`; `origin_hash` names the
    /// exact record when known; `recovers`: the quarantined record a recovered copy stands for
    /// (plan 4B Decision 27).
    CherryPick {
        origin: String,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovers: Option<String>,
    },
    /// Reverts the merge group `reverts`.
    Revert {
        reverts: String,
    },
    /// `recovers`: the quarantined record a recovered checkpoint stands for.
    Checkpoint {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovers: Option<String>,
    },
    /// Creates branch `branch_id` forking from this record's branch at `fork_seq` (whose
    /// txHash is `fork_hash`, absent at 0), committed at lease epoch `epoch`. `recovers`: the
    /// quarantined `branch` record a recovered branch stands for.
    Branch {
        branch_id: String,
        name: String,
        fork_seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fork_hash: Option<String>,
        epoch: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovers: Option<String>,
    },
    /// Renames, archives or restores branch `branch_id`. `recovers`: the quarantined
    /// `branchSet` record a recovered one stands for.
    BranchSet {
        branch_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archived: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovers: Option<String>,
    },
    /// Drops (and undrops) origin roots of branch `from` for this record's branch: a merge
    /// into it leaves a dropped origin out (plan 5E Decisions 1–3). Each list sorted and
    /// deduplicated, at least one non-empty, no id in both ([`Link::new_drop`]).
    Drop {
        from: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        drops: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        undrops: Vec<String>,
    },
}

impl Link {
    /// A `drop` link with `drops` and `undrops` each sorted and deduplicated. `INVALID_OP`
    /// when both are empty, they name more than [`MAX_DROP_IDS`] ids together, or an id is in
    /// both (`details.txId`).
    pub fn new_drop(
        from: impl Into<String>,
        mut drops: Vec<String>,
        mut undrops: Vec<String>,
    ) -> Result<Link, CoreError> {
        for l in [&mut drops, &mut undrops] {
            l.sort();
            l.dedup();
        }
        if drops.is_empty() && undrops.is_empty() {
            return Err(CoreError::new(
                "INVALID_OP",
                "a drop names at least one transaction to drop or undrop",
            )
            .with_fix("name the transactions (their origin txIds) to drop or undrop"));
        }
        let n = drops.len() + undrops.len();
        if n > MAX_DROP_IDS {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("a drop names at most {MAX_DROP_IDS} transactions, not {n}"),
            )
            .with_details(json!({ "count": n, "max": MAX_DROP_IDS }))
            .with_fix("split the drop into several"));
        }
        if let Some(both) = drops.iter().find(|d| undrops.binary_search(d).is_ok()) {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("transaction {both} is both dropped and undropped"),
            )
            .with_details(json!({ "txId": both }))
            .with_fix("name each transaction in one list only"));
        }
        Ok(Link::Drop {
            from: from.into(),
            drops,
            undrops,
        })
    }

    pub fn kind(&self) -> RecordKind {
        match self {
            Link::Edit => RecordKind::Edit,
            Link::Undo { .. } => RecordKind::Undo,
            Link::Redo { .. } => RecordKind::Redo,
            Link::Merge { .. } => RecordKind::Merge,
            Link::CherryPick { .. } => RecordKind::CherryPick,
            Link::Revert { .. } => RecordKind::Revert,
            Link::Checkpoint { .. } => RecordKind::Checkpoint,
            Link::Branch { .. } => RecordKind::Branch,
            Link::BranchSet { .. } => RecordKind::BranchSet,
            Link::Drop { .. } => RecordKind::Drop,
        }
    }
}

/// The kind fields of a record, in record order.
#[derive(Default, PartialEq)]
struct Fields {
    undoes: Option<Vec<String>>,
    redoes: Option<String>,
    merge_id: Option<String>,
    from: Option<String>,
    origin: Option<String>,
    origin_hash: Option<String>,
    reverts: Option<String>,
    name: Option<String>,
    branch_id: Option<String>,
    fork_seq: Option<u64>,
    fork_hash: Option<String>,
    epoch: Option<u64>,
    archived: Option<bool>,
    recovers: Option<String>,
    drops: Option<Vec<String>>,
    undrops: Option<Vec<String>>,
}

impl Fields {
    fn of(link: Link) -> Fields {
        let mut f = Fields::default();
        match link {
            Link::Edit => {}
            Link::Undo { undoes } => f.undoes = Some(undoes),
            Link::Redo { redoes } => f.redoes = Some(redoes),
            Link::Merge {
                merge_id,
                from,
                origin,
            } => {
                f.merge_id = Some(merge_id);
                f.from = Some(from);
                f.origin = Some(origin);
            }
            Link::CherryPick {
                origin,
                from,
                origin_hash,
                recovers,
            } => {
                f.origin = Some(origin);
                f.from = Some(from);
                f.origin_hash = origin_hash;
                f.recovers = recovers;
            }
            Link::Revert { reverts } => f.reverts = Some(reverts),
            Link::Checkpoint { name, recovers } => {
                f.name = Some(name);
                f.recovers = recovers;
            }
            Link::Branch {
                branch_id,
                name,
                fork_seq,
                fork_hash,
                epoch,
                recovers,
            } => {
                f.branch_id = Some(branch_id);
                f.name = Some(name);
                f.fork_seq = Some(fork_seq);
                f.fork_hash = fork_hash;
                f.epoch = Some(epoch);
                f.recovers = recovers;
            }
            Link::BranchSet {
                branch_id,
                name,
                archived,
                recovers,
            } => {
                f.branch_id = Some(branch_id);
                f.name = name;
                f.archived = archived;
                f.recovers = recovers;
            }
            Link::Drop {
                from,
                drops,
                undrops,
            } => {
                f.from = Some(from);
                f.drops = (!drops.is_empty()).then_some(drops);
                f.undrops = (!undrops.is_empty()).then_some(undrops);
            }
        }
        f
    }

    /// The camelCase names of the fields that are set.
    fn present(&self) -> Vec<&'static str> {
        [
            ("undoes", self.undoes.is_some()),
            ("redoes", self.redoes.is_some()),
            ("mergeId", self.merge_id.is_some()),
            ("from", self.from.is_some()),
            ("origin", self.origin.is_some()),
            ("originHash", self.origin_hash.is_some()),
            ("reverts", self.reverts.is_some()),
            ("name", self.name.is_some()),
            ("branchId", self.branch_id.is_some()),
            ("forkSeq", self.fork_seq.is_some()),
            ("forkHash", self.fork_hash.is_some()),
            ("epoch", self.epoch.is_some()),
            ("archived", self.archived.is_some()),
            ("recovers", self.recovers.is_some()),
            ("drops", self.drops.is_some()),
            ("undrops", self.undrops.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }
}

impl Record {
    fn fields(&self) -> Fields {
        Fields {
            undoes: self.undoes.clone(),
            redoes: self.redoes.clone(),
            merge_id: self.merge_id.clone(),
            from: self.from.clone(),
            origin: self.origin.clone(),
            origin_hash: self.origin_hash.clone(),
            reverts: self.reverts.clone(),
            name: self.name.clone(),
            branch_id: self.branch_id.clone(),
            fork_seq: self.fork_seq,
            fork_hash: self.fork_hash.clone(),
            epoch: self.epoch,
            archived: self.archived,
            recovers: self.recovers.clone(),
            drops: self.drops.clone(),
            undrops: self.undrops.clone(),
        }
    }

    /// The record's kind and kind fields as one [`Link`]. `INVALID_FORMAT` when a required
    /// field is missing, a field of another kind is set, an `undo` names no unit, a `branch`
    /// has `forkHash` other than exactly when `forkSeq > 0`, or a kind without ops has ops.
    pub fn link(&self) -> Result<Link, CoreError> {
        let f = self.fields();
        let kind = kind_name(self.kind);
        let need = |field: &str| {
            invalid_format(
                "record",
                format!("{} {kind} record needs {field}", article(&kind)),
            )
        };
        let link = match self.kind {
            RecordKind::Edit => Link::Edit,
            RecordKind::Undo => Link::Undo {
                undoes: f.undoes.clone().ok_or_else(|| need("undoes"))?,
            },
            RecordKind::Redo => Link::Redo {
                redoes: f.redoes.clone().ok_or_else(|| need("redoes"))?,
            },
            RecordKind::Merge => Link::Merge {
                merge_id: f.merge_id.clone().ok_or_else(|| need("mergeId"))?,
                from: f.from.clone().ok_or_else(|| need("from"))?,
                origin: f.origin.clone().ok_or_else(|| need("origin"))?,
            },
            RecordKind::CherryPick => Link::CherryPick {
                origin: f.origin.clone().ok_or_else(|| need("origin"))?,
                from: f.from.clone().ok_or_else(|| need("from"))?,
                origin_hash: f.origin_hash.clone(),
                recovers: f.recovers.clone(),
            },
            RecordKind::Revert => Link::Revert {
                reverts: f.reverts.clone().ok_or_else(|| need("reverts"))?,
            },
            RecordKind::Checkpoint => Link::Checkpoint {
                name: f.name.clone().ok_or_else(|| need("name"))?,
                recovers: f.recovers.clone(),
            },
            RecordKind::Branch => Link::Branch {
                branch_id: f.branch_id.clone().ok_or_else(|| need("branchId"))?,
                name: f.name.clone().ok_or_else(|| need("name"))?,
                fork_seq: f.fork_seq.ok_or_else(|| need("forkSeq"))?,
                fork_hash: f.fork_hash.clone(),
                epoch: f.epoch.ok_or_else(|| need("epoch"))?,
                recovers: f.recovers.clone(),
            },
            RecordKind::BranchSet => Link::BranchSet {
                branch_id: f.branch_id.clone().ok_or_else(|| need("branchId"))?,
                name: f.name.clone(),
                archived: f.archived,
                recovers: f.recovers.clone(),
            },
            RecordKind::Drop => {
                check_drop_lists(f.drops.as_deref(), f.undrops.as_deref())?;
                Link::Drop {
                    from: f.from.clone().ok_or_else(|| need("from"))?,
                    drops: f.drops.clone().unwrap_or_default(),
                    undrops: f.undrops.clone().unwrap_or_default(),
                }
            }
        };
        let expected = Fields::of(link.clone());
        if expected != f {
            let wanted = expected.present();
            let extra: Vec<&str> = f
                .present()
                .into_iter()
                .filter(|n| !wanted.contains(n))
                .collect();
            return Err(invalid_format(
                "record",
                format!(
                    "{} {kind} record must not have {}",
                    article(&kind),
                    extra.join(", ")
                ),
            ));
        }
        match &link {
            Link::Undo { undoes } if undoes.is_empty() => {
                return Err(invalid_format("record", "an undo record names no unit"));
            }
            Link::Branch {
                fork_seq,
                fork_hash,
                ..
            } if (*fork_seq > 0) != fork_hash.is_some() => {
                return Err(invalid_format(
                    "record",
                    "a branch record has forkHash exactly when forkSeq is above 0",
                ));
            }
            _ => {}
        }
        if !self.kind.has_ops()
            && !(self.intent.ops.is_empty() && self.resolved.is_empty() && self.inverse.is_empty())
        {
            return Err(invalid_format(
                "record",
                format!("{} {kind} record carries no ops", article(&kind)),
            ));
        }
        Ok(link)
    }

    /// Makes the record `link`'s kind, with exactly `link`'s fields (others cleared).
    pub fn set_link(&mut self, link: Link) {
        self.kind = link.kind();
        let f = Fields::of(link);
        self.undoes = f.undoes;
        self.redoes = f.redoes;
        self.merge_id = f.merge_id;
        self.from = f.from;
        self.origin = f.origin;
        self.origin_hash = f.origin_hash;
        self.reverts = f.reverts;
        self.name = f.name;
        self.branch_id = f.branch_id;
        self.fork_seq = f.fork_seq;
        self.fork_hash = f.fork_hash;
        self.epoch = f.epoch;
        self.archived = f.archived;
        self.recovers = f.recovers;
        self.drops = f.drops;
        self.undrops = f.undrops;
    }
}

/// A stored `drop` record's lists as [`Link::new_drop`] writes them: at least one present,
/// none empty, each sorted and deduplicated, at most [`MAX_DROP_IDS`] together, no id in both.
/// `INVALID_FORMAT` otherwise.
fn check_drop_lists(drops: Option<&[String]>, undrops: Option<&[String]>) -> Result<(), CoreError> {
    let bad = |why: &str| Err(invalid_format("record", format!("a drop record {why}")));
    if drops.is_none() && undrops.is_none() {
        return bad("needs drops or undrops");
    }
    let (d, u) = (drops.unwrap_or_default(), undrops.unwrap_or_default());
    for l in [drops, undrops].into_iter().flatten() {
        if l.is_empty() {
            return bad("has an empty list");
        }
        if !l.windows(2).all(|w| w[0] < w[1]) {
            return bad("has a list that is not sorted and deduplicated");
        }
    }
    if d.len() + u.len() > MAX_DROP_IDS {
        return bad(&format!("names more than {MAX_DROP_IDS} transactions"));
    }
    if d.iter().any(|x| u.binary_search(x).is_ok()) {
        return bad("names a transaction in both drops and undrops");
    }
    Ok(())
}

/// "an" before a vowel, else "a".
fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// `kind`'s serialised name (`cherryPick`): the one source of the camelCase kind text.
pub fn kind_name(kind: RecordKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{kind:?}"),
    }
}
