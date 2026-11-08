//! `validateJobParams` and `jobKindVersions` (5C-1 plan, Decision 27): each kind's params are
//! decoded by its Rust type, registered by the family's crate in one static list.

use fin_jobs::KindParams;
use fin_jobs::kinds::find;
use napi_derive::napi;
use serde_json::{Value, json};

/// Every family's params, in registration order. Families append theirs (6A-2: `fin_media`).
static PARAMS: &[&[KindParams]] = &[
    fin_library::params::PARAMS,
    #[cfg(feature = "test-hooks")]
    fin_jobs::test_kinds::PARAMS,
    #[cfg(feature = "test-hooks")]
    fin_library::params_test::PARAMS,
];

/// The longest params JSON a job may carry: a `start` must fit a 1 MiB line (Decision 4)
/// beside its other fields, so longer params would make a job that can never run.
pub(crate) const MAX_PARAMS_BYTES: usize = 512 << 10;

/// `{ok: true, params}` (defaults filled) or `{ok: false, errors}`, each error naming the field
/// when it can. `len` is the JSON's length as given.
pub(crate) fn validate(kind: &str, version: u32, params: &Value, len: usize) -> Value {
    if len > MAX_PARAMS_BYTES {
        let error = format!("params: {len} bytes of JSON, over the {MAX_PARAMS_BYTES} limit");
        return json!({ "ok": false, "errors": [error] });
    }
    match find(PARAMS, kind, version) {
        None => json!({ "ok": false, "errors": [format!("no params type for {kind}@{version}")] }),
        Some(k) => match (k.validate)(params) {
            // Defaults may lengthen it: what the job carries is checked too.
            Ok(params) if params.to_string().len() > MAX_PARAMS_BYTES => {
                let error =
                    format!("params: over {MAX_PARAMS_BYTES} bytes once defaults are filled");
                json!({ "ok": false, "errors": [error] })
            }
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

/// The kinds and versions `validateJobParams` knows.
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
