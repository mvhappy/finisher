//! Prints the JSON Schema of the TypeScript contract to stdout: `fin-core`'s types plus the
//! envelope types the engine sends that live above it (fin-store's [`Generation`],
//! [`EngineEvent`], [`TxSetAside`]; fin-probe's [`Probe`]). `@finisher/core-types` is generated
//! from it (`packages/core-types/scripts/generate.mjs`). Here, not in `fin-core`, because
//! `fin-core` cannot depend on the crates above it.

use fin_probe::Probe;
use fin_store::{EngineEvent, Generation, TxSetAside};

fn main() {
    let schema = fin_core::json_schema_with(|g| {
        g.subschema_for::<fin_core::Link>();
        g.subschema_for::<fin_core::Conflict>();
        g.subschema_for::<Generation>();
        g.subschema_for::<EngineEvent>();
        g.subschema_for::<TxSetAside>();
        g.subschema_for::<Probe>();
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}
