//! Applies a policy to the calling process (spec §8.3): the Landlock ruleset, then
//! `PR_SET_NO_NEW_PRIVS` and `landlock_restrict_self`. The seccomp filter joins in Task 4.
//!
//! Landlock restricts the calling thread and the threads it creates afterwards. From ABI 8
//! (`TSYNC`) every thread of the process is restricted at once; below it the process must have
//! one thread, else the restriction would miss the others and the start is refused.

use ::landlock::{RestrictSelfAttr, RulesetCreated, RulesetStatus};

use super::landlock::{Abi, TSYNC_ABI, query, ruleset};
use crate::error::ApplyError;
use crate::policy::SandboxPolicy;

/// What was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub abi: Abi,
    /// Every thread was restricted (`TSYNC`), not only a single-threaded caller.
    pub all_threads: bool,
}

/// Restricts the calling process to `policy` at the kernel's ABI.
pub fn restrict_self(policy: &SandboxPolicy) -> Result<Applied, ApplyError> {
    let abi = query().abi()?;
    enforce(ruleset(policy, abi)?, abi)
}

/// Restricts the calling process to `policy` using only the rights of `abi` (≤ the
/// kernel's; tests run the same cases at ABI 3 and at the kernel's). Test builds only: a
/// shipped caller always applies the kernel's full ABI.
#[cfg(any(test, feature = "test-hooks"))]
pub fn restrict_self_at(policy: &SandboxPolicy, abi: Abi) -> Result<Applied, ApplyError> {
    let rs = ruleset(policy, abi)?;
    enforce(rs, abi)
}

/// `PR_SET_NO_NEW_PRIVS` and `landlock_restrict_self` for a built ruleset; anything short of
/// full enforcement is an error.
pub fn enforce(rs: RulesetCreated, abi: Abi) -> Result<Applied, ApplyError> {
    let all_threads = abi.0 >= TSYNC_ABI;
    let rs = if all_threads {
        rs.all_threads(true).map_err(|e| ApplyError::Os {
            call: "LANDLOCK_RESTRICT_SELF_TSYNC".into(),
            error: e.to_string(),
        })?
    } else {
        let threads = thread_count()?;
        if threads != 1 {
            return Err(ApplyError::Threads { threads });
        }
        rs
    };
    let st = rs.restrict_self().map_err(|e| ApplyError::Os {
        call: "landlock_restrict_self".into(),
        error: e.to_string(),
    })?;
    if st.ruleset != RulesetStatus::FullyEnforced || !st.no_new_privs {
        return Err(ApplyError::Os {
            call: "landlock_restrict_self".into(),
            error: format!("not fully enforced: {st:?}"),
        });
    }
    Ok(Applied { abi, all_threads })
}

/// The calling process's thread count (`/proc/self/status`, read before the restriction).
fn thread_count() -> Result<usize, ApplyError> {
    let fail = |error: String| ApplyError::Os {
        call: "/proc/self/status".into(),
        error,
    };
    let text = std::fs::read_to_string("/proc/self/status").map_err(|e| fail(e.to_string()))?;
    text.lines()
        .find_map(|l| l.strip_prefix("Threads:"))
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| fail("no Threads line".into()))
}
