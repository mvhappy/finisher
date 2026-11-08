//! `validateJobParams` and `jobKindVersions` (5C-1 plan, Decision 27): each kind's params are
//! decoded by its Rust type, registered by the family's crate in one static list.

use fin_jobs::KindParams;
use napi_derive::napi;
use serde_json::{Value, json};

/// Every family's params, in registration order. Families append theirs (6A-2: `fin_media`).
static PARAMS: &[&[KindParams]] = &[
    #[cfg(feature = "test-hooks")]
    fin_jobs::test_kinds::PARAMS,
];

fn find(kind: &str, version: u32) -> Option<&'static KindParams> {
    PARAMS
        .iter()
        .flat_map(|family| family.iter())
        .find(|k| k.kind == kind && k.version == version)
}

/// `{ok: true, params}` (defaults filled) or `{ok: false, errors}`, each error naming the field
/// when it can.
pub(crate) fn validate(kind: &str, version: u32, params: &Value) -> Value {
    match find(kind, version) {
        None => json!({ "ok": false, "errors": [format!("no params type for {kind}@{version}")] }),
        Some(k) => match (k.validate)(params) {
            Ok(params) => json!({ "ok": true, "params": params }),
            Err(errors) => json!({ "ok": false, "errors": errors }),
        },
    }
}

/// A kind and version `validateJobParams` knows.
#[napi(object)]
pub struct JobKindVersion {
    pub kind: String,
    pub version: u32,
}

/// The kinds and versions `validateJobParams` knows (release builds: none yet).
#[napi(catch_unwind)]
pub fn job_kind_versions() -> Vec<JobKindVersion> {
    PARAMS
        .iter()
        .flat_map(|family| family.iter())
        .map(|k| JobKindVersion {
            kind: k.kind.to_owned(),
            version: k.version,
        })
        .collect()
}
