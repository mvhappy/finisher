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
    /// When set, the only branches a write may change (`BRANCH_NOT_ALLOWED` otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_branches: Option<Vec<String>>,
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
        assert_eq!(
            serde_json::to_value(WriteGuard::default()).unwrap(),
            json!({ "denyBranches": [] })
        );
    }
}
