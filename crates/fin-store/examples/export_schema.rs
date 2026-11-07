//! Prints the JSON Schema of the TypeScript contract to stdout: `fin-core`'s types plus the
//! envelope types the engine sends that live above it (fin-store's [`Generation`],
//! [`EngineEvent`], [`TxSetAside`], [`HistoryResult`], [`MergeReport`], [`RecoverReport`];
//! fin-probe's [`Probe`]; fin-fonts' [`FontCatalog`], [`FileState`], [`FontsStatus`],
//! [`FontsPlan`]). `FramePlan`, `AudioPlan`, `TextLayout` and `ParamSpec` come from fin-core, and
//! so do the diagnostics event types (`events::Envelope`, `AnyPayload`, the kind-keyed
//! `EventPayload` and every payload type, by `events::kinds::register_schemas`), and the
//! journal's (`fin-journal`: `PrivacySettings`, `ConsentState`, `JournalStatus`, `Filter`,
//! `PurgeSelector`, `PurgeReport`, `ProjectPolicy`; `CollectPolicy` is fin-core's).
//! `@finisher/core-types` is generated from it (`packages/core-types/scripts/generate.mjs`).
//! Here, not in `fin-core`, because `fin-core` cannot depend on the crates above it.

use fin_fonts::{FileState, FontCatalog, FontsPlan, FontsStatus};
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
        g.subschema_for::<fin_core::TextLayout>();
        g.subschema_for::<FontCatalog>();
        g.subschema_for::<FileState>();
        g.subschema_for::<FontsStatus>();
        g.subschema_for::<FontsPlan>();
        g.subschema_for::<fin_core::events::Envelope>();
        g.subschema_for::<fin_core::events::AnyPayload>();
        g.subschema_for::<fin_core::events::EventPayload>();
        fin_core::events::kinds::register_schemas(g);
        g.subschema_for::<fin_core::events::CollectPolicy>();
        g.subschema_for::<fin_journal::PrivacySettings>();
        g.subschema_for::<fin_journal::ConsentState>();
        g.subschema_for::<fin_journal::JournalStatus>();
        g.subschema_for::<fin_journal::Filter>();
        g.subschema_for::<fin_journal::PurgeSelector>();
        g.subschema_for::<fin_journal::PurgeReport>();
        g.subschema_for::<fin_journal::ProjectPolicy>();
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}
