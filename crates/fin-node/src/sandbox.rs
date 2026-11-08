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
