//! `sandboxStatus()` (plan 7C Task 3): every sandbox profile's status on this host, as
//! `fin_sandbox::SandboxStatus` JSON. The one status source `finisherd` reads
//! (`apps/finisherd/src/sandbox/status.ts`); `FIN_TEST_SANDBOX_STATUS` stubs it in test-hooks
//! builds.

// napi registers the export; a `cargo test` build has no caller.
#![cfg_attr(test, allow(dead_code))]

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Every profile's sandbox status (`SandboxStatus` JSON). Cheap: one Landlock version query.
#[napi]
pub fn sandbox_status() -> Result<String> {
    crate::encode(&fin_sandbox::status::all())
}

/// The session's git configuration text (`data/linux/git-config`, plan 7C Task 11):
/// `finisherd` writes it to `<session>/git/config`, which `GIT_CONFIG_GLOBAL` names.
#[napi]
pub fn sandbox_session_git_config() -> String {
    fin_sandbox::data::session_git_config().to_owned()
}

/// The base toolchain read list of this OS (`data/<os>/toolchain-base.txt`): on Linux and
/// macOS each entry's links resolved, missing and nested entries dropped (the form a policy
/// takes); on Windows the list as written (`%SystemRoot%` unexpanded).
#[napi]
pub fn sandbox_toolchain_base() -> Vec<String> {
    let os = fin_sandbox::Os::current();
    #[cfg(unix)]
    let list: Vec<String> = fin_sandbox::data::toolchain_base_resolved(os)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    #[cfg(not(unix))]
    let list: Vec<String> = fin_sandbox::data::toolchain_base(os)
        .into_iter()
        .map(str::to_owned)
        .collect();
    list
}

/// Checks an `agent` profile input (`AgentInput` JSON) the way the launcher will: `null` when
/// `fin_sandbox::profile::agent` builds a policy from it, else the reason it does not. So a
/// session whose input is wrong fails when its sandbox opens, not at each process it starts.
#[napi]
pub fn sandbox_agent_check(input: String) -> Option<String> {
    match serde_json::from_str::<fin_sandbox::AgentInput>(&input) {
        Err(e) => Some(format!("the agent input does not parse: {e}")),
        Ok(i) => fin_sandbox::profile::agent(&i).err().map(|e| e.to_string()),
    }
}
