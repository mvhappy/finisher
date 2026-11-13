//! Prints the JSON Schema of the TypeScript contract to stdout: `fin-core`'s types plus the
//! envelope types the engine sends (and `Tx.via`'s `Via`, by name) that live above it (fin-store's
//! [`Generation`], [`EngineEvent`], [`TxSetAside`], [`HistoryResult`], [`MergeReport`],
//! [`HistoryLog`], [`DropReq`] (the one history request whose shape the contracts take from here, plan 5E),
//! [`ReviewPlan`] (with fin-core's `ReviewEntry`, plan 5E Decision 7), [`RecoverReport`],
//! the status fin-node serves ([`ProjectStatusView`]: [`ProjectStatus`] and the font summary),
//! the [`VerifyReport`], [`RebuildReport`] and [`CompactReport`] of the long calls,
//! and the [`WriteGuard`] finisherd sends with every write;
//! fin-probe's [`Probe`]; fin-fonts' [`FontCatalog`], [`FileState`], [`FontsStatus`],
//! [`FontsPlan`]). `FramePlan`, `AudioPlan`, `TextLayout` and `ParamSpec` come from fin-core, and
//! so do the diagnostics event types (`events::Envelope`, `AnyPayload`, the kind-keyed
//! `EventPayload` and every payload type, by `events::kinds::register_schemas`), and the
//! journal's (`fin-journal`: `PrivacySettings`, `ConsentState`, `JournalStatus`, `Filter`,
//! `PurgeSelector`, `PurgeReport`, `ProjectPolicy`, `DiagnosticsReport`, `ExportPage`;
//! `CollectPolicy` is fin-core's), the media types of `fin-media` (`MediaInfo` and its
//! parts; the media job params, inputs and results of `fin_media::jobs`, sub-project 6),
//! the job envelope of `fin-jobs` with the test kinds' params and
//! results (`JobDone` and `WorkerExit` are fin-core payloads, registered with the others),
//! the JSON shapes of fin-node's `JobHost` (`fin-jobhost`'s `wire`),
//! the plugin model's (`fin-plugin-api`: `Manifest`, `Permission`, `PermissionDiff`, `Risk`,
//! `LicenceClass`, `ContributionKind`, `Invalid`, the `commands` and `hooks` contributions,
//! the argument schema subset's `SchemaSubset`, `SchemaError` and `ArgProblem`, and the plugin
//! protocol's context, command, call, rights and error types, by `schema::register_schemas`) and its
//! store's (`fin-registry`: `InstallPlan`, `InstalledPlugin`, `Accept`, `ValidationReport`,
//! `TargetCustom`), the sandbox profiles' typed inputs (`fin-sandbox`: `AgentInput`,
//! `HookInput`, `WorkerInput`, `PluginInput`, `SandboxProfile`; sub-project 7C), and
//! the library indexer's wire types of `fin-library` (`RawRel`, `Expect`, `ScanEntry`,
//! `DirDone`, `ScanBatch`, `KnownDir`, `VolumeIdentity`, `RootState`, `Placeholder`, the
//! `library` family's params, results and calls; sub-project 5D).
//! `@finisher/core-types` is generated from it (`packages/core-types/scripts/generate.mjs`).
//! Here, not in `fin-core`, because `fin-core` cannot depend on the crates above it.

use fin_fonts::{FileState, FontCatalog, FontsPlan, FontsStatus};
use fin_probe::Probe;
use fin_store::{
    BranchCreated, BranchView, CompactReport, DropReq, EngineEvent, Generation, HistoryLog,
    HistoryResult, MergeReport, ProjectStatus, ProjectStatusView, RebuildReport, RecoverGroup,
    RecoverReport, ReviewPlan, SkippedFile, TxSetAside, VerifyReport, WriteGuard,
};

fn main() {
    let schema = fin_core::json_schema_with(|g| {
        g.subschema_for::<fin_core::Link>();
        // Reached through `Tx.via` too; named so the plugin SDKs can import it (plan 8B Task 3).
        g.subschema_for::<fin_core::tx::Via>();
        g.subschema_for::<fin_core::Conflict>();
        g.subschema_for::<Generation>();
        g.subschema_for::<fin_core::branch::BranchState>();
        g.subschema_for::<BranchView>();
        g.subschema_for::<BranchCreated>();
        g.subschema_for::<HistoryResult>();
        g.subschema_for::<MergeReport>();
        g.subschema_for::<HistoryLog>();
        g.subschema_for::<DropReq>();
        g.subschema_for::<ReviewPlan>();
        g.subschema_for::<RecoverGroup>();
        g.subschema_for::<SkippedFile>();
        g.subschema_for::<RecoverReport>();
        g.subschema_for::<fin_core::history::Stacks>();
        g.subschema_for::<EngineEvent>();
        g.subschema_for::<TxSetAside>();
        g.subschema_for::<WriteGuard>();
        g.subschema_for::<ProjectStatus>();
        g.subschema_for::<ProjectStatusView>();
        g.subschema_for::<VerifyReport>();
        g.subschema_for::<RebuildReport>();
        g.subschema_for::<CompactReport>();
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
        g.subschema_for::<fin_journal::DiagnosticsReport>();
        g.subschema_for::<fin_journal::ExportPage>();
        g.subschema_for::<fin_media::MediaInfo>();
        g.subschema_for::<fin_media::jobs::LookParams>();
        g.subschema_for::<fin_media::jobs::LookResult>();
        g.subschema_for::<fin_media::jobs::CapabilitiesParams>();
        g.subschema_for::<fin_media::jobs::LegacyMediaCapabilities>();
        g.subschema_for::<fin_media::jobs::MediaCapabilities>();
        g.subschema_for::<fin_media::jobs::JobSnapshot>();
        g.subschema_for::<fin_media::jobs::RenderManifest>();
        // The media cache (6B-1 Task 13): `meta.json`, the key input, `media.cache.status|gc`.
        g.subschema_for::<fin_media::cache::EntryMeta>();
        g.subschema_for::<fin_media::cache::KeyInput>();
        g.subschema_for::<fin_media::cache::CacheStatus>();
        g.subschema_for::<fin_media::cache::CacheGcResult>();
        g.subschema_for::<fin_media::jobs::ProbeDeepParams>();
        g.subschema_for::<fin_media::jobs::ProbeDeepResult>();
        // `media.proxy` (6B-1 Task 14): params, result, `media.proxy.status`.
        g.subschema_for::<fin_media::jobs::ProxyParams>();
        g.subschema_for::<fin_media::jobs::ProxyResult>();
        g.subschema_for::<fin_media::jobs::ProxyStatus>();
        // `media.thumbs` (6B-1 Task 15): params, an item's value, the filmstrip index, a run.
        g.subschema_for::<fin_media::jobs::ThumbsParams>();
        g.subschema_for::<fin_media::jobs::ThumbItem>();
        g.subschema_for::<fin_media::jobs::StripIndex>();
        g.subschema_for::<fin_media::jobs::ThumbsRun>();
        g.subschema_for::<fin_library::RawRel>();
        g.subschema_for::<fin_library::Expect>();
        g.subschema_for::<fin_library::ScanEntry>();
        g.subschema_for::<fin_library::DirDone>();
        g.subschema_for::<fin_library::ScanBatch>();
        g.subschema_for::<fin_library::KnownDir>();
        g.subschema_for::<fin_library::VolumeIdentity>();
        g.subschema_for::<fin_library::RootState>();
        g.subschema_for::<fin_library::Placeholder>();
        fin_library::params::register_schemas(g);
        fin_jobs::register_schemas(g);
        fin_jobs::test_kinds::register_schemas(g);
        fin_jobhost::wire::register_schemas(g);
        fin_plugin_api::schema::register_schemas(g);
        fin_registry::schema::register_schemas(g);
        fin_sandbox::register_schemas(g);
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serializes")
    );
}
