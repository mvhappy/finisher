//! `fin-jobs` (5C-1 plan, Decisions 3–9a): the job envelope (Rust → JSON Schema → generated
//! TypeScript), its framing ([`codec`]) and the worker and helper runtimes that every family
//! links ([`worker`], [`helper`]), with host calls and the self-sandbox entry ([`sandbox`]).
//! It creates no process; the host side is `fin-jobhost`.

use std::sync::OnceLock;

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::Value;

pub mod budget;
pub mod codec;
pub mod error;
pub mod helper;
pub mod kinds;
pub mod msg_host;
pub mod msg_worker;
pub mod sandbox;
#[cfg(feature = "test-hooks")]
pub mod test_kinds;
pub mod worker;

pub use budget::ReadBudget;
pub use error::{ItemError, JobError, Retry};
pub use kinds::KindParams;
pub use msg_host::{
    Attach, CancelReason, Dirs, FileGrant, FileMode, HelperGrant, HelperKind, HostMsg, Limits,
    Start, StartJob,
};
pub use msg_worker::{Hello, KindVersion, Progress, ProgressUnit, Role, WorkerMsg};

/// The protocol major (`v`).
pub const PROTOCOL: u32 = 1;
/// The protocol minor this tree speaks (Decision 3).
pub const MINOR: u32 = 0;
/// A runtime's exit code when its host is gone (stdin EOF).
pub const EXIT_HOST_GONE: i32 = 70;
/// A runtime's exit code when a requested self-sandbox cannot be applied (Decision 9a).
pub const EXIT_SELF_SANDBOX: i32 = 78;
/// The default `alive` interval before the first `start`.
pub const DEFAULT_ALIVE_MS: u32 = 5000;

/// Both directions of the envelope, for its schema and id.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Envelope {
    host: HostMsg,
    worker: WorkerMsg,
}

/// The envelope's JSON Schema (draft 2020-12): both directions.
#[doc(hidden)]
pub fn envelope_schema() -> Value {
    let generator = SchemaGenerator::new(SchemaSettings::draft2020_12());
    serde_json::to_value(generator.into_root_schema_for::<Envelope>()).unwrap_or(Value::Null)
}

/// The id of a schema: the first 16 hex digits of BLAKE3 over its canonical JSON (keys
/// sorted, `description`s dropped, so a doc comment does not change it).
#[doc(hidden)]
pub fn envelope_id_of(schema: &Value) -> String {
    let mut out = Vec::new();
    canonical(schema, &mut out);
    blake3::hash(&out).to_hex()[..16].to_owned()
}

fn canonical(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map
                .keys()
                .filter(|k| !(*k == "description" && map[*k].is_string()))
                .collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(Value::String((*k).clone()).to_string().as_bytes());
                out.push(b':');
                canonical(&map[*k], out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                canonical(item, out);
            }
            out.push(b']');
        }
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

/// This tree's envelope id (Decision 3), computed once per process. A bundled process must
/// send exactly this in `hello.envelope`.
pub fn envelope_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| envelope_id_of(&envelope_schema()))
}

/// Adds the envelope's types to `g` (`fin-store`'s `export_schema`, for the generated
/// TypeScript): both message unions, their parts and [`sandbox::SelfSandbox`].
pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<HostMsg>();
    g.subschema_for::<WorkerMsg>();
    g.subschema_for::<Hello>();
    g.subschema_for::<Start>();
    g.subschema_for::<Attach>();
    g.subschema_for::<Progress>();
    g.subschema_for::<JobError>();
    g.subschema_for::<ItemError>();
    g.subschema_for::<sandbox::SelfSandbox>();
}
