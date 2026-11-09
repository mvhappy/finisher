//! The in-transaction write guard (sub-project 5 spec §4.4 stage 6; plan 5A Decision 17):
//! which branches a write may change, by id, as `finisherd`'s policy stage computed them.
//!
//! [`Store::with_guard`] sets a guard for one call; [`Store::write`] copies it into its
//! [`WriteCtx`](crate::write::WriteCtx), and every operation resolves the branch it writes to
//! with [`WriteCtx::target`](crate::write::WriteCtx::target) (or checks a branch it changes
//! by another branch's log, a `branchSet`, with
//! [`WriteCtx::guarded`](crate::write::WriteCtx::guarded)). The check therefore runs inside
//! the IMMEDIATE transaction, after the store resolved the reference (a name, `name#id` or
//! an id, as the branch index holds it at that moment) and before anything is planned: a
//! rename or a racing policy change between the policy stage and the write cannot open a
//! window, and a refusal rolls back and writes nothing.
//!
//! A new branch is not a target: a `branch` record goes on its parent's log, which it only
//! reads (the fork), and the branch did not exist when the guard was computed, so it is in
//! no list. Recovery makes its branches the same way and copies records only onto them; a
//! `branchSet` it applies again to a live branch is checked as [`Store::set_branch`] is.
//!
//! **Credentials** (plan 5A Task 9; review of Task 7, M3): a write made for an `agent` or
//! `mcp` credential names it (`credential: "<kind>:<id>"`). `finisherd` ends a credential
//! with [`end_credential`] before it closes the credential's connections, and
//! [`Store::write`] checks the write's credential while it holds [`live_credential`]'s read
//! lock, **through the SQLite commit**: [`end_credential`] takes the write lock, so it
//! returns only once every write already past the check has committed, and every write
//! that checks after it is refused (`CREDENTIAL_REVOKED`) and writes nothing. A revoked
//! credential's write already handed to a worker thread therefore cannot land after the
//! revocation. The set is per process and only grows (credential ids are ULIDs, never
//! reused); a credential revoked before this process started cannot prove a connection, so
//! it needs no entry.

use std::collections::HashSet;
use std::sync::{LazyLock, PoisonError, RwLock, RwLockReadGuard};

use fin_core::CoreError;
use fin_core::branch::BranchInfo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Store, StoreError};

/// The branches a write may change, by branch id (`br<n>`, `main`). The default guards
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteGuard {
    /// Branches no write may change (`BRANCH_PROTECTED`).
    #[serde(default)]
    pub deny_branches: Vec<String>,
    /// When set, the only branches a write may change (`BRANCH_NOT_ALLOWED` otherwise). Absent
    /// means no allow-list; `null` is refused, so a sender's bug cannot open the guard.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "allow_list"
    )]
    #[schemars(with = "Vec<String>")]
    pub allow_branches: Option<Vec<String>>,
    /// The credential the write is made for (`agent:<id>`, `mcp:<id>`), refused once it has
    /// been ended ([`end_credential`]); absent for `local` and `console`. `null` is refused.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "some_string"
    )]
    #[schemars(with = "String")]
    pub credential: Option<String>,
}

/// `credential` when present: a string, never `null`.
fn some_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

/// Credentials ended in this process (module docs).
static ENDED: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// Ends `credential` (`<kind>:<id>`) for every store of this process: a write that checks it
/// from now on is refused, and this returns only once every write already past its check has
/// committed (module docs).
pub fn end_credential(credential: &str) {
    ENDED
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(credential.to_string());
}

/// Whether `credential` was ended in this process.
pub fn credential_ended(credential: &str) -> bool {
    ENDED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(credential)
}

/// The read lock a write holds from its credential check through its commit, or
/// `CREDENTIAL_REVOKED` when the guard's credential was ended. A guard without a credential
/// holds nothing. [`Store::write`] holds it; so does every other change a credential can make
/// to a store outside it (`fin-node`'s `acknowledgeRewind`, `acknowledgeSalvage` and
/// `setCollect`; review of 5A Tasks 8–9, L1). Hold it only for a short change: ending a
/// credential waits for every holder, on `finisherd`'s main thread.
pub fn live_credential(
    guard: &WriteGuard,
) -> Result<Option<RwLockReadGuard<'static, HashSet<String>>>, StoreError> {
    let Some(credential) = &guard.credential else {
        return Ok(None);
    };
    let ended = ENDED.read().unwrap_or_else(PoisonError::into_inner);
    if ended.contains(credential) {
        return Err(CoreError::new(
            "CREDENTIAL_REVOKED",
            "the credential this write was made for was revoked",
        )
        .with_fix(
            "This credential no longer works. Ask the owner of this Finisher server for a new \
             one.",
        )
        .into());
    }
    Ok(Some(ended))
}

/// `allowBranches` when present: a list, never `null`.
fn allow_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(d).map(Some)
}

/// Whether `guard` lets a write change `target` (a branch the store resolved): a denied one
/// is `BRANCH_PROTECTED`, one outside an allow-list `BRANCH_NOT_ALLOWED`; both with
/// `details {branch, name}` and a fix that addresses the user.
pub(crate) fn check(guard: &WriteGuard, target: &BranchInfo) -> Result<(), StoreError> {
    let details = json!({ "branch": target.id, "name": target.name });
    if guard.deny_branches.contains(&target.id) {
        return Err(CoreError::new(
            "BRANCH_PROTECTED",
            format!("branch {}#{} is protected", target.name, target.id),
        )
        .with_details(details)
        .with_fix(
            "This branch is protected: changes reach it through a review. Write to another \
             branch and ask for a review.",
        )
        .into());
    }
    if let Some(allow) = &guard.allow_branches
        && !allow.contains(&target.id)
    {
        return Err(CoreError::new(
            "BRANCH_NOT_ALLOWED",
            format!(
                "this credential may not write to branch {}#{}",
                target.name, target.id
            ),
        )
        .with_details(details)
        .with_fix(
            "This credential may only write to the branches it was given. Write to one of \
             those, or ask the owner of this Finisher server for a credential that covers \
             this branch.",
        )
        .into());
    }
    Ok(())
}

impl Store {
    /// Runs `f` with `guard` checked by every write it makes (module docs), then puts the
    /// previous guard back, also when `f` fails or panics. A nested call replaces the outer
    /// guard for its duration; it does not add to it.
    pub fn with_guard<R>(&mut self, guard: WriteGuard, f: impl FnOnce(&mut Store) -> R) -> R {
        let previous = std::mem::replace(&mut self.guard, guard);
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.guard = previous;
        match out {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Runs `f` with `when` as what its undos, redos and copies do with an item whose
    /// neighbours are both gone (`fin_core::rebase::placement`; owner decision 2026-11-09):
    /// `Snap` for a person, `Refuse` for an agent. The caller decides from who asked (fin-node:
    /// the call's actor); the default, outside such a call, is `Snap`. The previous value is
    /// put back, also when `f` fails or panics.
    pub fn with_when_lost<R>(
        &mut self,
        when: fin_core::rebase::placement::WhenLost,
        f: impl FnOnce(&mut Store) -> R,
    ) -> R {
        let previous = std::mem::replace(&mut self.when_lost, when);
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        self.when_lost = previous;
        match out {
            Ok(r) => r,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, name: &str) -> BranchInfo {
        BranchInfo {
            id: id.into(),
            name: name.into(),
            parent: None,
            fork_seq: 0,
            fork_hash: None,
            created_seq: None,
            created_by: String::new(),
            created_at: 0,
            record: None,
            epoch: 0,
            archived: false,
            merged: false,
        }
    }

    #[test]
    fn default_guard_allows_everything() {
        assert!(check(&WriteGuard::default(), &info("main", "main")).is_ok());
    }

    #[test]
    fn deny_wins_over_allow() {
        let g = WriteGuard {
            deny_branches: vec!["br1".into()],
            allow_branches: Some(vec!["br1".into()]),
            credential: None,
        };
        let e = CoreError::from(check(&g, &info("br1", "feature")).unwrap_err());
        assert_eq!(e.code, "BRANCH_PROTECTED");
        assert_eq!(e.details, json!({ "branch": "br1", "name": "feature" }));
    }

    #[test]
    fn allow_list_is_by_id() {
        let g = WriteGuard {
            deny_branches: vec![],
            allow_branches: Some(vec!["feature".into()]),
            credential: None,
        };
        let e = CoreError::from(check(&g, &info("br1", "feature")).unwrap_err());
        assert_eq!(e.code, "BRANCH_NOT_ALLOWED");
        let g = WriteGuard {
            allow_branches: Some(vec!["br1".into()]),
            ..g
        };
        assert!(check(&g, &info("br1", "feature")).is_ok());
    }

    #[test]
    fn fix_texts_contain_no_command() {
        let g = WriteGuard {
            deny_branches: vec!["br1".into()],
            allow_branches: Some(vec![]),
            credential: None,
        };
        for target in [info("br1", "a"), info("br2", "b")] {
            let e = CoreError::from(check(&g, &target).unwrap_err());
            let fix = e.fix.expect("a fix");
            assert!(!fix.contains('`'), "{fix}");
            assert!(
                !fix.contains("fin ") && !fix.contains("finisherd "),
                "{fix}"
            );
        }
    }

    #[test]
    fn wire_shape() {
        let g: WriteGuard = serde_json::from_str(r#"{"allowBranches":["br2"]}"#).unwrap();
        assert_eq!(g.deny_branches, Vec::<String>::new());
        assert_eq!(g.allow_branches, Some(vec!["br2".to_string()]));
        assert!(serde_json::from_str::<WriteGuard>(r#"{"deny":["br2"]}"#).is_err());
        // `null` would read as no allow-list: refused (review of Task 5, L2).
        assert!(serde_json::from_str::<WriteGuard>(r#"{"allowBranches":null}"#).is_err());
        assert_eq!(
            serde_json::from_str::<WriteGuard>("{}").unwrap(),
            WriteGuard::default()
        );
        assert_eq!(
            serde_json::to_value(WriteGuard::default()).unwrap(),
            json!({ "denyBranches": [] })
        );
        let g: WriteGuard = serde_json::from_str(r#"{"credential":"agent:01X"}"#).unwrap();
        assert_eq!(g.credential.as_deref(), Some("agent:01X"));
        assert!(serde_json::from_str::<WriteGuard>(r#"{"credential":null}"#).is_err());
    }

    #[test]
    fn ended_credential_is_refused() {
        let g = WriteGuard {
            credential: Some("agent:01GUARDUNITTEST0000000000".into()),
            ..WriteGuard::default()
        };
        assert!(live_credential(&g).unwrap().is_some());
        assert!(live_credential(&WriteGuard::default()).unwrap().is_none());
        end_credential("agent:01GUARDUNITTEST0000000000");
        assert!(credential_ended("agent:01GUARDUNITTEST0000000000"));
        let e = CoreError::from(live_credential(&g).unwrap_err());
        assert_eq!(e.code, "CREDENTIAL_REVOKED");
        assert!(!e.fix.unwrap().contains('`'));
    }
}
