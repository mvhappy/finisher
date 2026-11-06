//! Prints the JSON Schema of the TypeScript contract to stdout: `fin-core`'s types plus the
//! envelope types the engine sends that live above it (fin-store's [`Generation`],
//! [`EngineEvent`], [`TxSetAside`], [`HistoryResult`], [`MergeReport`], [`RecoverReport`];
//! fin-probe's [`Probe`]).
//! `@finisher/core-types` is generated from it (`packages/core-types/scripts/generate.mjs`).
//! Here, not in `fin-core`, because `fin-core` cannot depend on the crates above it.

use fin_probe::Probe;
use fin_store::{
    BranchCreated, BranchView, EngineEvent, Generation, HistoryResult, MergeReport, RecoverGroup,
    RecoverReport, SkippedFile, TxSetAside,
};

fn main() {
    let schema = fin_core::json_schema_with(|g| {
        g.subschema_for::<fin_core::Link>();
        g.subschema_for::<fin_core::Conflict>();
        g.subschema_for::<Generation>();
        g.subschema_for::<fin_core::branch::BranchState>();
        g.subschema_for::<BranchView>();
        g.subschema_for::<BranchCreated>();
        g.subschema_for::<HistoryResult>();
        g.subschema_for::<MergeReport>();
        g.subschema_for::<RecoverGroup>();
        g.subschema_for::<SkippedFile>();
        g.subschema_for::<RecoverReport>();
        g.subschema_for::<fin_core::history::Stacks>();
        g.subschema_for::<EngineEvent>();
        g.subschema_for::<TxSetAside>();
        g.subschema_for::<Probe>();
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}
