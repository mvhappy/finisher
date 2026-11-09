//! `validateJobParams` and `jobKindVersions` (5C-1 plan, Decision 27): each kind's params are
//! decoded by its Rust type, registered by the family's crate in one static list.

use fin_jobs::KindParams;
use fin_jobs::kinds::{find, schema_as, validate_as};
use fin_media::jobs::{CapabilitiesParams, LookParams};
use napi_derive::napi;
use serde_json::{Value, json};

/// Every family's params, in registration order. Families append theirs (6A-2: `fin_media`).
static PARAMS: &[&[KindParams]] = &[
    fin_library::params::PARAMS,
    #[cfg(feature = "test-hooks")]
    fin_jobs::test_kinds::PARAMS,
    #[cfg(feature = "test-hooks")]
    fin_library::params_test::PARAMS,
    MEDIA,
    PLUGIN,
];

/// Sub-project 8B's plugin kinds: a command run (`fin_plugin_api::protocol::CommandRun`; the
/// kind itself is registered by finisherd's plugin host, Task 9).
static PLUGIN: &[KindParams] = &[KindParams {
    kind: "plugin.command",
    version: 1,
    validate: validate_as::<fin_plugin_api::protocol::CommandRun>,
    schema: schema_as::<fin_plugin_api::protocol::CommandRun>,
}];

/// Sub-project 6's media kinds (`fin_media::jobs`; the worker side is `fin-workers media`).
static MEDIA: &[KindParams] = &[
    KindParams {
        kind: "media.look",
        version: 1,
        validate: validate_look,
        schema: schema_as::<LookParams>,
    },
    KindParams {
        kind: "media.capabilities",
        version: 1,
        validate: validate_as::<CapabilitiesParams>,
        schema: schema_as::<CapabilitiesParams>,
    },
];

/// `LookParams`, then the checks serde cannot make (counts, sizes, the output name), as the
/// worker makes them.
fn validate_look(params: &Value) -> Result<Value, Vec<String>> {
    let filled = validate_as::<LookParams>(params)?;
    let p: LookParams = serde_json::from_value(filled.clone()).map_err(|e| vec![e.to_string()])?;
    p.check().map_err(|e| vec![e.message])?;
    // The plan samples are the host's: `prepare` computes them from the snapshot it writes. A
    // submitter's would only ever be ignored (the worker prefers the kind's), so say so.
    if !p.plan_samples.is_empty() {
        return Err(vec![
            "params.planSamples: computed by the kind's prepare; leave it out".to_string(),
        ]);
    }
    Ok(filled)
}

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

/// The kinds and versions `validateJobParams` knows (release builds: the library and media kinds).
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
