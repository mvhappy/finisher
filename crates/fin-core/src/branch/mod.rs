//! Branches (spec §4.1; plan 4B Decisions 11–14): the branch index folded from `branch` and
//! `branchSet` records, branch references, and the name rules.
//!
//! A branch is `main` or `br<n>` (from the shared id counter). Its display name is unique,
//! case-insensitively, among the branches that are not archived; `merged` branches keep
//! theirs. Two active branches can still share a name when both arrived through import: the
//! one with the lower numeric id holds it (`br9` before `br10`), and the others list as
//! `<name>#<id>`. A **reference** is an id, the name of a branch that holds it, or
//! `<name>#<id>` (which also reaches an archived branch).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::record::{Link, Record};

/// The root branch: the only one without a parent, never renamed or archived.
pub const MAIN: &str = "main";

/// Longest branch or checkpoint name, in bytes (names are ASCII).
pub const NAME_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BranchState {
    Active,
    Archived,
    Merged,
}

/// One branch as the index holds it (one `branches` row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub id: String,
    pub name: String,
    /// `None` for `main`.
    pub parent: Option<String>,
    /// The parent's seq the branch forks at; its own seqs start right after.
    pub fork_seq: u64,
    /// The parent's `txHash` at `fork_seq` (`None` at 0).
    pub fork_hash: Option<String>,
    /// The parent's seq of the `branch` record (`None` for `main`).
    pub created_seq: Option<u64>,
    /// The `branch` record's author (`""` for `main`).
    pub created_by: String,
    /// The `branch` record's `committedAt` (0 for `main`).
    pub created_at: i64,
    pub archived: bool,
    pub merged: bool,
}

impl BranchInfo {
    /// Archived over merged over active.
    pub fn state(&self) -> BranchState {
        if self.archived {
            BranchState::Archived
        } else if self.merged {
            BranchState::Merged
        } else {
            BranchState::Active
        }
    }

    /// The `main` row, as a new project and migration 0002 hold it.
    pub fn main() -> BranchInfo {
        BranchInfo {
            id: MAIN.into(),
            name: MAIN.into(),
            parent: None,
            fork_seq: 0,
            fork_hash: None,
            created_seq: None,
            created_by: String::new(),
            created_at: 0,
            archived: false,
            merged: false,
        }
    }
}

/// The number of a `br<n>` id (`"br10"` → 10), for numeric ordering; `None` for `main` and
/// anything else.
pub fn id_number(id: &str) -> Option<u64> {
    let digits = id.strip_prefix("br")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `main` first, then `br<n>` by number, then anything else by text.
fn order_key(id: &str) -> (u8, u64, &str) {
    match (id == MAIN, id_number(id)) {
        (true, _) => (0, 0, id),
        (false, Some(n)) => (1, n, id),
        (false, None) => (2, 0, id),
    }
}

/// Whether `name` reads as a branch id (`br` and digits, any case).
fn looks_like_id(name: &str) -> bool {
    name.len() > 2
        && name.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("br"))
        && name[2..].bytes().all(|b| b.is_ascii_digit())
}

fn invalid_name(kind: &str, name: &str, why: &str) -> CoreError {
    CoreError::new("INVALID_NAME", format!("{kind} name {name:?} {why}"))
        .with_details(json!({ "kind": kind, "name": name, "reason": why }))
        .with_fix(format!(
            "use 1-{NAME_MAX} letters, digits, '.', '_' or '-', not starting with '-' or '.', \
             and not \"main\" or an id like \"br1\""
        ))
}

fn name_rules(kind: &str, name: &str) -> Result<(), CoreError> {
    if name.is_empty() {
        return Err(invalid_name(kind, name, "is empty"));
    }
    if name.len() > NAME_MAX {
        return Err(invalid_name(
            kind,
            name,
            &format!("is longer than {NAME_MAX} characters"),
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid_name(
            kind,
            name,
            "may hold only letters, digits, '.', '_' and '-'",
        ));
    }
    if name.starts_with(['-', '.']) {
        return Err(invalid_name(kind, name, "must not start with '-' or '.'"));
    }
    if name.eq_ignore_ascii_case(MAIN) {
        return Err(invalid_name(kind, name, "is reserved for the main branch"));
    }
    if looks_like_id(name) {
        return Err(invalid_name(kind, name, "looks like a branch id"));
    }
    Ok(())
}

/// A branch display name (Decision 11): `[A-Za-z0-9._-]{1,64}`, not starting with `-` or `.`,
/// not `main` and not like an id (`br\d+`), all case-insensitively. `INVALID_NAME`
/// (`details {kind: "branch", name, reason}`) otherwise.
pub fn validate_branch_name(name: &str) -> Result<(), CoreError> {
    name_rules("branch", name)
}

/// A checkpoint name: the branch name rules, and not all digits (a seq).
pub fn validate_checkpoint_name(name: &str) -> Result<(), CoreError> {
    name_rules("checkpoint", name)?;
    if name.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid_name(
            "checkpoint",
            name,
            "is all digits, like a seq",
        ));
    }
    Ok(())
}

fn malformed(rec: &Record, why: String) -> CoreError {
    CoreError::new(
        "INVALID_FORMAT",
        format!("record {} does not fit the branch index: {why}", rec.tx_id),
    )
    .with_details(json!({ "kind": "record", "txId": rec.tx_id, "reason": why }))
}

/// The branches of a project, by id, folded from the log (module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchIndex {
    by_id: BTreeMap<String, BranchInfo>,
}

impl BranchIndex {
    /// The index of a project with no branch records: `main` alone.
    pub fn with_main() -> BranchIndex {
        BranchIndex::from_infos([])
    }

    /// An index of `infos` (the `branches` table), plus `main` when they lack it.
    pub fn from_infos(infos: impl IntoIterator<Item = BranchInfo>) -> BranchIndex {
        let mut by_id: BTreeMap<String, BranchInfo> =
            infos.into_iter().map(|i| (i.id.clone(), i)).collect();
        by_id
            .entry(MAIN.to_string())
            .or_insert_with(BranchInfo::main);
        BranchIndex { by_id }
    }

    pub fn get(&self, id: &str) -> Option<&BranchInfo> {
        self.by_id.get(id)
    }

    /// Folds the record at `seq` of `branch` into the index: a `branch` record adds its
    /// branch (a child of `branch`), a `branchSet` renames, archives or restores a child of
    /// `branch`; other kinds change nothing. Returns the id of the branch it changed. A
    /// record the log of a project never holds (an id already known, a branch record on an
    /// unknown branch or forking at or above its own seq, a `forkHash` present when
    /// `forkSeq` is 0 or absent when it is not (Decision 3), a name the name rules refuse
    /// ([`validate_branch_name`]: it would not resolve or list back), a `branchSet` of a
    /// branch that is not a child of `branch`) is `INVALID_FORMAT`, and the index is left as
    /// it was.
    pub fn absorb(
        &mut self,
        branch: &str,
        seq: u64,
        rec: &Record,
    ) -> Result<Option<String>, CoreError> {
        match rec.link()? {
            Link::Branch {
                branch_id,
                name,
                fork_seq,
                fork_hash,
            } => {
                if !self.by_id.contains_key(branch) {
                    return Err(malformed(
                        rec,
                        format!("it is on unknown branch {branch:?}"),
                    ));
                }
                if id_number(&branch_id).is_none() {
                    return Err(malformed(rec, format!("{branch_id:?} is not a branch id")));
                }
                if self.by_id.contains_key(&branch_id) {
                    return Err(malformed(rec, format!("branch {branch_id} already exists")));
                }
                if fork_seq >= seq {
                    return Err(malformed(
                        rec,
                        format!("it forks at seq {fork_seq}, not below its own seq {seq}"),
                    ));
                }
                if fork_hash.is_some() != (fork_seq > 0) {
                    return Err(malformed(
                        rec,
                        format!("forkHash must be present exactly when forkSeq ({fork_seq}) > 0"),
                    ));
                }
                validate_branch_name(&name).map_err(|e| malformed(rec, e.message))?;
                let info = BranchInfo {
                    id: branch_id.clone(),
                    name,
                    parent: Some(branch.to_string()),
                    fork_seq,
                    fork_hash,
                    created_seq: Some(seq),
                    created_by: rec.author.clone(),
                    created_at: rec.committed_at,
                    archived: false,
                    merged: false,
                };
                self.by_id.insert(branch_id.clone(), info);
                Ok(Some(branch_id))
            }
            Link::BranchSet {
                branch_id,
                name,
                archived,
            } => {
                if let Some(name) = &name {
                    validate_branch_name(name).map_err(|e| malformed(rec, e.message))?;
                }
                let Some(info) = self.by_id.get_mut(&branch_id) else {
                    return Err(malformed(rec, format!("branch {branch_id} is unknown")));
                };
                if info.parent.as_deref() != Some(branch) {
                    return Err(malformed(
                        rec,
                        format!("branch {branch_id} is not a child of {branch}"),
                    ));
                }
                if let Some(name) = name {
                    info.name = name;
                }
                if let Some(archived) = archived {
                    info.archived = archived;
                }
                Ok(Some(branch_id))
            }
            _ => Ok(None),
        }
    }

    /// The non-archived branch that holds `name` (case-insensitively): of several, the lowest
    /// numeric id.
    pub fn holder(&self, name: &str) -> Option<&BranchInfo> {
        self.by_id
            .values()
            .filter(|i| !i.archived && i.name.eq_ignore_ascii_case(name))
            .min_by(|a, b| order_key(&a.id).cmp(&order_key(&b.id)))
    }

    /// The branch a reference names: an id, the name a non-archived branch holds
    /// (case-insensitively), or `<name>#<id>` (any state).
    pub fn resolve(&self, r: &str) -> Option<&BranchInfo> {
        if let Some(info) = self.by_id.get(r) {
            return Some(info);
        }
        if let Some((name, id)) = r.rsplit_once('#') {
            return self
                .by_id
                .get(id)
                .filter(|i| i.name.eq_ignore_ascii_case(name));
        }
        self.holder(r)
    }

    /// Whether a non-archived branch (merged ones included) holds `name`.
    pub fn name_taken(&self, name: &str) -> bool {
        self.holder(name).is_some()
    }

    /// The reference a listing shows for `info`: its name while that resolves to it, else
    /// `<name>#<id>` (an archived branch, or the second holder of a duplicate name).
    pub fn display(&self, info: &BranchInfo) -> String {
        if self.holder(&info.name).is_some_and(|h| h.id == info.id) {
            info.name.clone()
        } else {
            format!("{}#{}", info.name, info.id)
        }
    }

    /// Every branch with its display reference, `main` first, then by numeric id.
    pub fn list(&self) -> Vec<(String, &BranchInfo)> {
        let mut all: Vec<&BranchInfo> = self.by_id.values().collect();
        all.sort_by(|a, b| order_key(&a.id).cmp(&order_key(&b.id)));
        all.into_iter().map(|i| (self.display(i), i)).collect()
    }

    /// The ancestors of `id`, parent first (stops at a loop or an unknown parent).
    fn ancestors(&self, id: &str) -> Vec<&str> {
        let mut out: Vec<&str> = vec![];
        let mut cur = self.by_id.get(id).and_then(|i| i.parent.as_deref());
        while let Some(p) = cur {
            if p == id || out.contains(&p) {
                break;
            }
            out.push(p);
            cur = self.by_id.get(p).and_then(|i| i.parent.as_deref());
        }
        out
    }

    /// Whether `a` descends from `of` (a child, grandchild, …; never itself).
    pub fn is_descendant(&self, a: &str, of: &str) -> bool {
        self.ancestors(a).contains(&of)
    }

    /// Every id, parents before children: by depth, then numeric id.
    pub fn parents_first(&self) -> Vec<&str> {
        let mut all: Vec<(usize, &str)> = self
            .by_id
            .keys()
            .map(|id| (self.ancestors(id).len(), id.as_str()))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0).then(order_key(a.1).cmp(&order_key(b.1))));
        all.into_iter().map(|(_, id)| id).collect()
    }
}
