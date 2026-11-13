import type {
  CommittedTx,
  EngineEvent,
  Project,
  Tx,
  TxSetAside,
} from "@finisher/core-types";
import { Schema } from "effect";
import { Rpc, RpcGroup } from "effect/rpc";
import { TxSetAsideSchema } from "./tx-set-aside.ts";
import {
  CommittedTxSchema,
  CompactReportSchema,
  ProbeSchema,
  ProjectStatusViewSchema,
  RebuildReportSchema,
  VerifyReportSchema,
} from "./generated/schemas.ts";
import { DEVICE_RPCS } from "./device-rpcs.ts";
import { REVIEW_RPCS } from "./review-rpcs.ts";
import {
  AudioPlanSchema,
  EvalAudioPayload,
  EvalFramePayload,
  FontCatalogSchema,
  FontStatusSchema,
  FontsFetchPayload,
  FontsPlanPayload,
  FontsPlanSchema,
  FramePlanSchema,
  MediaCapabilitiesSuccess,
  MediaLookPayload,
  MediaLookSuccess,
} from "./media.ts";
import {
  AgentApprovalAnswerPayload,
  AgentApprovalAnswerSuccess,
  AgentApprovalWaitPayload,
  AgentApprovalWaitSuccess,
  AgentApprovalListPayload,
  AgentApprovalListSuccess,
  AgentProvidersSuccess,
  AgentSelfSuccess,
  AgentSelfTryPayload,
  AgentSessionControlPayload,
  AgentSessionControlSuccess,
  AgentSessionDeletePayload,
  AgentSessionDeleteSuccess,
  AgentSessionGetPayload,
  AgentSessionGetSuccess,
  AgentSessionListPayload,
  AgentSessionListSuccess,
  AgentSessionPromptPayload,
  AgentSessionPromptSuccess,
  AgentSessionStartPayload,
  AgentSessionStartSuccess,
  AgentSessionSteerPayload,
  AgentSessionSteerSuccess,
  AgentSessionWaitPayload, AgentSessionWaitSuccess,
  AgentSessionRollbackPayload, AgentSessionRollbackSuccess,
  AgentWatchPayload,
  WatchItem,
} from "./agent.ts";
import {
  AgentProjectTrustPayload,
  AgentProjectTrustSuccess,
  AgentRulesAddPayload,
  AgentRulesAddSuccess,
  AgentRulesListPayload,
  AgentRulesListSuccess,
  AgentRulesRemovePayload, AgentRulesRemoveSuccess,
} from "./rules.ts";
import { AgentHooksListPayload, AgentHooksListSuccess } from "./hooks.ts";
import { AgentSavedPromptListPayload, AgentSavedPromptListSuccess, AgentSavedPromptRunPayload, AgentSavedPromptRunSuccess } from "./saved-prompts.ts";
import {
  AgentKeyListSuccess,
  AgentKeyRemovePayload,
  AgentKeyRemoveSuccess,
  AgentKeySetPayload,
  AgentKeySetSuccess,
  AgentLocalRemovePayload,
  AgentLocalRemoveSuccess,
  AgentLocalSetPayload,
  AgentLocalSetSuccess,
  AgentLocalShowSuccess,
  AgentLocalTestSuccess,
  TrainingClosedTargetsSetPayload,
  TrainingClosedTargetsSuccess,
} from "./agent-admin.ts";
import {
  AuthProvePayload,
  AuthProveSuccess,
  AuthTicketPayload,
  AuthTicketSuccess,
  AuthWhoamiSuccess,
  AuditListPayload,
  AuditListSuccess,
  AuditPurgePayload,
  AuditPurgeSuccess,
  ChallengeField,
  ConsoleChallengeSuccess,
  ServerHelloPayload, ServerHelloSuccess,
  ServerStatusSuccess, ServerStopPayload, ServerStopSuccess,
} from "./auth.ts";
import {
  CredentialListPayload,
  CredentialListSuccess,
  CredentialMintPayload,
  CredentialMintSuccess,
  CredentialRevokePayload,
  CredentialRevokeSuccess,
  CredentialRotateMasterPayload,
  CredentialRotateMasterSuccess,
} from "./credentials.ts";
import { FinError } from "./errors.ts";
import {
  JobIdPayload,
  JobsListOfViews,
  JobsListPayload,
  JobsListSuccess,
  JobsSubmitPayload,
  JobsTargetPayload,
  JobsTargetSuccess,
  JobsWaitPayload,
  JobsWatchPayload,
  JobsWorkersSuccess,
  JobWatchEventSchema,
} from "./job-rpcs.ts";
import { JobViewSchema } from "./jobs.ts";
import {
  BranchCreatePayload, BranchCreateSuccess, BranchListPayload, BranchListSuccess, BranchSetPayload,
  HistoryCheckpointPayload, HistoryCherryPickPayload, HistoryLogPayload, HistoryLogSuccess,
  HistoryMergePayload, HistoryRedoPayload, HistoryResultSuccess, HistoryRevertPayload, HistoryUndoPayload,
  MergeReportSuccess, ProjectRecoverPayload, RecordSuccess, RecoverReportSuccess,
} from "./history.ts";
import {
  PluginInfoPayload,
  PluginInfoSuccess,
  PluginListPayload,
  PluginListSuccess,
  PluginSearchPayload,
  PluginSearchSuccess,
  RegistryAddPayload,
  RegistryForgetPayload,
  RegistryIdPayload,
  RegistryRemovePayload,
  RegistryListSuccess,
  RegistryRefreshPayload,
  RegistryRefreshSuccess,
  RegistryStatusSuccess,
} from "./plugins.ts";
import {
  PluginDiscardPayload,
  PluginDiscardSuccess,
  PluginStatePayload,
  PluginInstallPayload,
  PluginInstallSuccess,
  PluginPlanPayload,
  PluginPlanSuccess,
  PluginRemovePayload,
  PluginRemoveSuccess,
  PluginStateSuccess,
  PluginSuggestPayload,
  PluginSuggestSuccess,
} from "./plugin-lifecycle.ts";
import { PluginContentGetPayload, PluginContentGetSuccess } from "./plugin-content.ts";
import { PluginCommandRunPayload, PluginCommandRunSuccess, PluginCommandsPayload, PluginCommandsSuccess } from "./plugin-commands.ts";
import {
  LibraryAddPayload,
  LibraryAddSuccess,
  LibraryFilePayload,
  LibraryFileSuccess,
  LibraryFilesPayload,
  LibraryFilesSuccess,
  LibraryListSuccess,
  LibraryLookupPayload,
  LibraryLookupSuccess,
  LibraryRemovePayload,
  LibraryRemoveSuccess,
  LibraryScanPayload,
  LibraryScanSuccess,
  LibrarySearchPayload,
  LibrarySearchSuccess,
  LibrarySetPayload,
  LibrarySetSuccess,
  LibraryStatusPayload,
  LibraryStatusSuccess,
  LibraryWatchEventSchema,
  LibraryWatchPayload,
} from "./library.ts";
import {
  DiagnosticsConsentPayload,
  DiagnosticsConsentSuccess,
  DiagnosticsExportPayload,
  DiagnosticsExportSuccess,
  DiagnosticsProjectPayload,
  DiagnosticsProjectSuccess,
  DiagnosticsPurgePayload,
  DiagnosticsPurgeSuccess,
  DiagnosticsRecordPayload,
  DiagnosticsRecordSuccess,
  DiagnosticsStatusPayload,
  DiagnosticsStatusSuccess,
} from "./diagnostics.ts";

/**
 * Envelope types generated from the Rust types they carry (`@finisher/core-types`). The Effect
 * schemas below that validate them at runtime are checked against the same generated schema
 * (`envelopes.test.ts`).
 */
export type {
  AudioPlan,
  EngineEvent,
  FileState,
  FontCatalog,
  FontFileStatus,
  FontsPlan,
  FontsStatus,
  FramePlan,
  Generation,
  Probe,
  TxSetAside,
} from "@finisher/core-types";

/** Narrowed `Schema.Unknown` payloads (Rust validates). A sent tx has no `via`; a stored one may. */
export type TxPayload = Omit<Tx, "via">;
export type ProjectPayload = Project;
export type CommittedTxPayload = CommittedTx;


/**
 * `idemKey`: makes a retry of this create (its answer lost) return the project it made rather
 * than `PROJECT_EXISTS`; the same key with another name or rate is `IDEM_KEY_REUSED`.
 */
export const ProjectCreatePayload = Schema.Struct({
  dir: Schema.String,
  name: Schema.String,
  rate: Schema.String,
  idemKey: Schema.optional(Schema.String),
});
/**
 * `relocate`: the project now lives in `dir` although the folder it was recorded at cannot be
 * reached (`PROJECT_ORIGINAL_UNREACHABLE` otherwise). A person's decision; never set it
 * automatically.
 */
export const ProjectOpenPayload = Schema.Struct({
  dir: Schema.String,
  relocate: Schema.optional(Schema.Boolean),
  /** The console's challenge code of a relocation (5A Decision 30). */
  challenge: ChallengeField,
});
export const ProjectRefSuccess = Schema.Struct({
  projectId: Schema.String,
  rev: Schema.Number,
});

/** Protocol 2: the probe is for `projectId` (reach: in that project's folder for an agent or MCP principal; a device's allowed roots, 5B Decision 32). */
export const AssetProbePayload = Schema.Struct({ projectId: Schema.String, path: Schema.String });
/**
 * What `asset.probe` answers: fin-probe's `Probe` (generated, as its schema): the asset kind
 * its bytes show and that kind's fields, absent ones omitted; for MP4/MOV `media`, fin-media's
 * `MediaInfo` (display size, rotation, colour, HDR, VFR; never in `asset.add`).
 */
export { ProbeSchema };
export const AssetProbeSuccess = ProbeSchema;

/**
 * What a client read a branch at: the database's `incarnation` (new after every rebuild) and
 * the branch's rewind count `n`. `snapshot.get` and `project.status` (`incarnation` plus
 * `branches[].generation`) report it.
 */
export const GenerationSchema = Schema.Struct({
  incarnation: Schema.String,
  n: Schema.Number,
});

/**
 * `generation`: the generation the client read the branch at (from `snapshot.get`). With it
 * the commit is refused with `HISTORY_REWOUND` when that history was rewound or the database
 * rebuilt since, instead of being rebased over a history the client never saw. Required since
 * protocol 2 (spec §3.6).
 */
export const TxCommitPayload = Schema.Struct({
  projectId: Schema.String,
  tx: Schema.Unknown,
  generation: GenerationSchema,
});
/** A committed record, passed through (fin-core validated it), typed as the generated `CommittedTx`. */
export { CommittedTxSchema };

export const SnapshotGetPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
});
export const SnapshotGetSuccess = Schema.Struct({
  rev: Schema.Number,
  project: Schema.Unknown,
  /** Read together with `rev`: pass it to `tx.commit`. */
  generation: Schema.optional(GenerationSchema),
});

/** Every project-scoped storage RPC takes just the project. */
export const ProjectIdPayload = Schema.Struct({ projectId: Schema.String });
/**
 * fin-store's `ProjectStatusView`: the `ProjectStatus` with the server's font summary (`fonts`)
 * and, from `project.status`, `busy: null | { operation, sinceMs }` (generated).
 */
export const ProjectStatusSchema = ProjectStatusViewSchema;
export const ProjectTakeoverPayload = Schema.Struct({
  projectId: Schema.String,
  /** Needed on sync folders, which never take over without a person's say-so. */
  confirm: Schema.Boolean,
  /** The console's challenge code of `confirm: true` (5A Decision 30). */
  challenge: ChallengeField,
});
export const ProjectAcknowledgeRewindPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  /** The generation the rewind started (`warnings[].details.generation` in the status). */
  generation: Schema.Number,
});
export const ProjectAcknowledgeSalvagePayload = Schema.Struct({
  projectId: Schema.String,
  /** The kept copy's file name (`SALVAGE_INCOMPLETE` warning). */
  copy: Schema.String,
});
/** `acknowledged`: false when nothing matching was listed. */
export const AcknowledgeSuccess = Schema.Struct({ acknowledged: Schema.Boolean });

/** One `project.events` item. */
export const ProjectEventSchema = Schema.Union([
  /** The status changed (first item: the status now). */
  Schema.Struct({ type: Schema.Literal("status"), status: ProjectStatusSchema }),
  /** An import took `branch`'s revs from `fromRev` on out of its history. */
  Schema.Struct({
    type: Schema.Literal("rewound"),
    branch: Schema.String,
    fromRev: Schema.Number,
    quarantined: Schema.Number,
    own: Schema.Boolean,
    generation: Schema.Number,
    incarnation: Schema.String,
  }),
  /** The database was rebuilt: every rev read before may hold something else now. */
  Schema.Struct({ type: Schema.Literal("rebuilt"), incarnation: Schema.String }),
  /** A background tick, backup or compaction failed (`fatal`: the engine is gone). */
  Schema.Struct({ type: Schema.Literal("error"), error: Schema.Unknown, fatal: Schema.Boolean }),
]);
/**
 * fin-store's `EngineEvent` less `applied` (fed to `ops.subscribe`), `lagged` (ends subscribers)
 * and `collect` (the project's collection flag, for the journal only).
 */
export type ProjectEvent = Exclude<EngineEvent, { type: "applied" } | { type: "lagged" } | { type: "collect" }>;

export { TxSetAsideSchema };
export * from "./media.ts";
export const TxGetPayload = Schema.Struct({ projectId: Schema.String, txId: Schema.String });
/** `tx.get`: the `CommittedTx`, a set-aside record, or `null` when the server never saw it. */
export const TxGetSuccess = Schema.NullOr(Schema.Union([TxSetAsideSchema, CommittedTxSchema]));
export type TxGetResult = CommittedTx | TxSetAside | null;

/**
 * `generation`: what the caller read `fromRev` at (from `snapshot.get`). When the branch was
 * rewound or the database rebuilt since, the stream ends at once with `HISTORY_REWOUND`
 * instead of continuing a history the caller never saw. Required since protocol 2.
 */
export const OpsSubscribePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  fromRev: Schema.Number,
  generation: GenerationSchema,
});


export const FinRpcs = RpcGroup.make(
  Rpc.make("server.hello", {
    payload: ServerHelloPayload,
    success: ServerHelloSuccess,
    error: FinError,
  }),
  // The handshake's second half (5A Decision 9): binds the hello's credential to the connection.
  Rpc.make("auth.prove", { payload: AuthProvePayload, success: AuthProveSuccess, error: FinError }),
  // 5A Task 6: one-time WebSocket tickets, the caller's principal, a credential revoking itself.
  Rpc.make("auth.ticket", { payload: AuthTicketPayload, success: AuthTicketSuccess, error: FinError }),
  Rpc.make("auth.whoami", { payload: Schema.Struct({}), success: AuthWhoamiSuccess, error: FinError }),
  Rpc.make("auth.revokeSelf", { payload: Schema.Struct({}), success: Schema.Struct({}), error: FinError }),
  Rpc.make("console.challenge", { payload: Schema.Struct({}), success: ConsoleChallengeSuccess, error: FinError }),
  // 5A Task 8: the server's own state and graceful stop; the owner's audit log.
  Rpc.make("server.status", { payload: Schema.Struct({}), success: ServerStatusSuccess, error: FinError }),
  Rpc.make("server.stop", { payload: ServerStopPayload, success: ServerStopSuccess, error: FinError }),
  Rpc.make("auth.audit.list", { payload: AuditListPayload, success: AuditListSuccess, error: FinError }),
  Rpc.make("auth.audit.purge", { payload: AuditPurgePayload, success: AuditPurgeSuccess, error: FinError }),
  // 5A Task 9: the owner's agent and MCP credentials.
  Rpc.make("auth.credential.mint", { payload: CredentialMintPayload, success: CredentialMintSuccess, error: FinError }),
  Rpc.make("auth.credential.list", { payload: CredentialListPayload, success: CredentialListSuccess, error: FinError }),
  Rpc.make("auth.credential.revoke", { payload: CredentialRevokePayload, success: CredentialRevokeSuccess, error: FinError }),
  Rpc.make("auth.credential.rotateMaster", { payload: CredentialRotateMasterPayload, success: CredentialRotateMasterSuccess, error: FinError }),
  Rpc.make("project.create", { payload: ProjectCreatePayload, success: ProjectRefSuccess, error: FinError }),
  Rpc.make("project.open", { payload: ProjectOpenPayload, success: ProjectRefSuccess, error: FinError }),
  /** Review #11: closes the engine now (flushed, lease released); `false` when it was not open. The next call reopens it. */
  Rpc.make("project.close", { payload: ProjectIdPayload, success: Schema.Struct({ closed: Schema.Boolean }), error: FinError }),
  Rpc.make("asset.probe", {
    payload: AssetProbePayload,
    success: AssetProbeSuccess,
    error: FinError,
  }),
  Rpc.make("tx.commit", {
    payload: TxCommitPayload,
    success: CommittedTxSchema,
    error: FinError,
  }),
  Rpc.make("snapshot.get", {
    payload: SnapshotGetPayload,
    success: SnapshotGetSuccess,
    error: FinError,
  }),
  Rpc.make("ops.subscribe", {
    payload: OpsSubscribePayload,
    success: CommittedTxSchema,
    error: FinError,
    stream: true,
  }),
  Rpc.make("project.status", { payload: ProjectIdPayload, success: ProjectStatusSchema, error: FinError }),
  Rpc.make("project.takeover", { payload: ProjectTakeoverPayload, success: ProjectStatusSchema, error: FinError }),
  /** fin-store's `VerifyReport`: `{ ok, checks: [{ name, ok, details }] }`. */
  Rpc.make("project.verify", { payload: ProjectIdPayload, success: VerifyReportSchema, error: FinError }),
  Rpc.make("project.rebuild", { payload: ProjectIdPayload, success: RebuildReportSchema, error: FinError }),
  /** fin-store's `CompactReport` (a forced run). */
  Rpc.make("project.compact", { payload: ProjectIdPayload, success: CompactReportSchema, error: FinError }),
  /** fin-store's `RecoverReport`: the groups found, the branches made, every record written. A long call. */
  Rpc.make("project.recover", { payload: ProjectRecoverPayload, success: RecoverReportSuccess, error: FinError }),
  /** Plan 4B Task 18: branches and history. Every committed record reaches its branch's subscribers. */
  Rpc.make("branch.create", { payload: BranchCreatePayload, success: BranchCreateSuccess, error: FinError }),
  Rpc.make("branch.list", { payload: BranchListPayload, success: BranchListSuccess, error: FinError }),
  Rpc.make("branch.set", { payload: BranchSetPayload, success: RecordSuccess, error: FinError }),
  Rpc.make("history.undo", { payload: HistoryUndoPayload, success: HistoryResultSuccess, error: FinError }),
  Rpc.make("history.redo", { payload: HistoryRedoPayload, success: HistoryResultSuccess, error: FinError }),
  Rpc.make("history.checkpoint", { payload: HistoryCheckpointPayload, success: RecordSuccess, error: FinError }),
  Rpc.make("history.merge", { payload: HistoryMergePayload, success: MergeReportSuccess, error: FinError }),
  Rpc.make("history.cherryPick", { payload: HistoryCherryPickPayload, success: HistoryResultSuccess, error: FinError }),
  Rpc.make("history.revert", { payload: HistoryRevertPayload, success: HistoryResultSuccess, error: FinError }),
  Rpc.make("history.log", { payload: HistoryLogPayload, success: HistoryLogSuccess, error: FinError }),
  Rpc.make("project.acknowledgeRewind", {
    payload: ProjectAcknowledgeRewindPayload,
    success: AcknowledgeSuccess,
    error: FinError,
  }),
  Rpc.make("project.acknowledgeSalvage", {
    payload: ProjectAcknowledgeSalvagePayload,
    success: AcknowledgeSuccess,
    error: FinError,
  }),
  /** Plan 4A Decision 4: status and history events on their own stream, not `ops.subscribe`. */
  Rpc.make("project.events", { payload: ProjectIdPayload, success: ProjectEventSchema, error: FinError, stream: true }),
  Rpc.make("tx.get", { payload: TxGetPayload, success: TxGetSuccess, error: FinError }),
  /** Plan 4C Task 22: evaluation and fonts. Edits keep going through `tx.commit`. */
  Rpc.make("eval.frame", { payload: EvalFramePayload, success: FramePlanSchema, error: FinError }),
  Rpc.make("eval.audio", { payload: EvalAudioPayload, success: AudioPlanSchema, error: FinError }),
  Rpc.make("fonts.catalog", { payload: Schema.Struct({}), success: FontCatalogSchema, error: FinError }),
  Rpc.make("fonts.status", { payload: Schema.Struct({}), success: FontStatusSchema, error: FinError }),
  Rpc.make("fonts.fetch", { payload: FontsFetchPayload, success: FontStatusSchema, error: FinError }),
  Rpc.make("fonts.plan", { payload: FontsPlanPayload, success: FontsPlanSchema, error: FinError }),
  /** Plan 6A Task 13: looks at a timeline, and what this machine's media worker can do. */
  Rpc.make("media.look", { payload: MediaLookPayload, success: MediaLookSuccess, error: FinError }),
  Rpc.make("media.capabilities", { payload: Schema.Struct({}), success: MediaCapabilitiesSuccess, error: FinError }),
  // 5C-2: the job engine (spec §6.7). Others' jobs and jobs out of reach are `JOB_NOT_FOUND`.
  Rpc.make("jobs.submit", { payload: JobsSubmitPayload, success: JobViewSchema, error: FinError }),
  Rpc.make("jobs.get", { payload: JobIdPayload, success: JobViewSchema, error: FinError }),
  Rpc.make("jobs.list", { payload: JobsListPayload, success: JobsListSuccess, error: FinError }),
  Rpc.make("jobs.wait", { payload: JobsWaitPayload, success: JobsListOfViews, error: FinError }),
  Rpc.make("jobs.watch", { payload: JobsWatchPayload, success: JobWatchEventSchema, error: FinError, stream: true }),
  Rpc.make("jobs.cancel", { payload: JobIdPayload, success: JobViewSchema, error: FinError }),
  Rpc.make("jobs.pause", { payload: JobsTargetPayload, success: JobsTargetSuccess, error: FinError }),
  Rpc.make("jobs.resume", { payload: JobsTargetPayload, success: JobsTargetSuccess, error: FinError }),
  Rpc.make("jobs.workers", { payload: Schema.Struct({}), success: JobsWorkersSuccess, error: FinError }),
  // Scope `diagnostics` (plan D, Decision 32).
  Rpc.make("diagnostics.status", { payload: DiagnosticsStatusPayload, success: DiagnosticsStatusSuccess, error: FinError }),
  Rpc.make("diagnostics.export", { payload: DiagnosticsExportPayload, success: DiagnosticsExportSuccess, error: FinError }),
  Rpc.make("diagnostics.purge", { payload: DiagnosticsPurgePayload, success: DiagnosticsPurgeSuccess, error: FinError }),
  Rpc.make("diagnostics.project", { payload: DiagnosticsProjectPayload, success: DiagnosticsProjectSuccess, error: FinError }),
  Rpc.make("diagnostics.consent", { payload: DiagnosticsConsentPayload, success: DiagnosticsConsentSuccess, error: FinError }),
  Rpc.make("diagnostics.record", { payload: DiagnosticsRecordPayload, success: DiagnosticsRecordSuccess, error: FinError }),
  // Plugins and registries (sub-project 8A Tasks 9a and 9b): the registries and what they
  // offer, then plans, approvals and the lifecycle.
  Rpc.make("registry.list", { payload: Schema.Struct({}), success: RegistryListSuccess, error: FinError }),
  Rpc.make("registry.status", { payload: RegistryIdPayload, success: RegistryStatusSuccess, error: FinError }),
  Rpc.make("registry.add", { payload: RegistryAddPayload, success: RegistryStatusSuccess, error: FinError }),
  Rpc.make("registry.remove", { payload: RegistryRemovePayload, success: Schema.Struct({ removed: Schema.Boolean }), error: FinError }),
  Rpc.make("registry.forget", { payload: RegistryForgetPayload, success: Schema.Struct({ forgotten: Schema.Boolean }), error: FinError }),
  Rpc.make("registry.refresh", { payload: RegistryRefreshPayload, success: RegistryRefreshSuccess, error: FinError }),
  Rpc.make("plugin.list", { payload: PluginListPayload, success: PluginListSuccess, error: FinError }),
  Rpc.make("plugin.search", { payload: PluginSearchPayload, success: PluginSearchSuccess, error: FinError }),
  Rpc.make("plugin.info", { payload: PluginInfoPayload, success: PluginInfoSuccess, error: FinError }),
  Rpc.make("plugin.content.get", { payload: PluginContentGetPayload, success: PluginContentGetSuccess, error: FinError }),
  Rpc.make("plugin.plan", { payload: PluginPlanPayload, success: PluginPlanSuccess, error: FinError }),
  Rpc.make("plugin.discard", { payload: PluginDiscardPayload, success: PluginDiscardSuccess, error: FinError }),
  Rpc.make("plugin.install", { payload: PluginInstallPayload, success: PluginInstallSuccess, error: FinError }),
  Rpc.make("plugin.remove", { payload: PluginRemovePayload, success: PluginRemoveSuccess, error: FinError }),
  Rpc.make("plugin.enable", { payload: PluginStatePayload, success: PluginStateSuccess, error: FinError }),
  Rpc.make("plugin.disable", { payload: PluginStatePayload, success: PluginStateSuccess, error: FinError }),
  Rpc.make("plugin.suggest", { payload: PluginSuggestPayload, success: PluginSuggestSuccess, error: FinError }),
  // Plugin commands (8B Task 9): the list and a run, committed under the invoker with `Tx.via`.
  Rpc.make("plugin.commands", { payload: PluginCommandsPayload, success: PluginCommandsSuccess, error: FinError }),
  Rpc.make("plugin.command.run", { payload: PluginCommandRunPayload, success: PluginCommandRunSuccess, error: FinError }),
  // Libraries (sub-project 5D Task 13): the owner's folders of originals, their catalogs and search.
  Rpc.make("library.add", { payload: LibraryAddPayload, success: LibraryAddSuccess, error: FinError }),
  Rpc.make("library.remove", { payload: LibraryRemovePayload, success: LibraryRemoveSuccess, error: FinError }),
  Rpc.make("library.set", { payload: LibrarySetPayload, success: LibrarySetSuccess, error: FinError }),
  Rpc.make("library.list", { payload: Schema.Struct({}), success: LibraryListSuccess, error: FinError }),
  Rpc.make("library.status", { payload: LibraryStatusPayload, success: LibraryStatusSuccess, error: FinError }),
  Rpc.make("library.files", { payload: LibraryFilesPayload, success: LibraryFilesSuccess, error: FinError }),
  Rpc.make("library.file", { payload: LibraryFilePayload, success: LibraryFileSuccess, error: FinError }),
  Rpc.make("library.search", { payload: LibrarySearchPayload, success: LibrarySearchSuccess, error: FinError }),
  Rpc.make("library.lookup", { payload: LibraryLookupPayload, success: LibraryLookupSuccess, error: FinError }),
  Rpc.make("library.scan", { payload: LibraryScanPayload, success: LibraryScanSuccess, error: FinError }),
  Rpc.make("library.watch", { payload: LibraryWatchPayload, success: LibraryWatchEventSchema, error: FinError, stream: true }),
  // The agent runtime (7A Task 2): sessions, the watch stream, approvals, the session's own state.
  Rpc.make("agent.session.start", { payload: AgentSessionStartPayload, success: AgentSessionStartSuccess, error: FinError }),
  Rpc.make("agent.session.prompt", { payload: AgentSessionPromptPayload, success: AgentSessionPromptSuccess, error: FinError }),
  Rpc.make("agent.session.steer", { payload: AgentSessionSteerPayload, success: AgentSessionSteerSuccess, error: FinError }),
  Rpc.make("agent.session.interrupt", { payload: AgentSessionControlPayload, success: AgentSessionControlSuccess, error: FinError }),
  Rpc.make("agent.session.resume", { payload: AgentSessionControlPayload, success: AgentSessionControlSuccess, error: FinError }),
  Rpc.make("agent.session.end", { payload: AgentSessionControlPayload, success: AgentSessionControlSuccess, error: FinError }),
  Rpc.make("agent.session.wait", { payload: AgentSessionWaitPayload, success: AgentSessionWaitSuccess, error: FinError }),
  Rpc.make("agent.session.rollback", { payload: AgentSessionRollbackPayload, success: AgentSessionRollbackSuccess, error: FinError }),
  Rpc.make("agent.session.list", { payload: AgentSessionListPayload, success: AgentSessionListSuccess, error: FinError }),
  Rpc.make("agent.session.get", { payload: AgentSessionGetPayload, success: AgentSessionGetSuccess, error: FinError }),
  Rpc.make("agent.session.delete", { payload: AgentSessionDeletePayload, success: AgentSessionDeleteSuccess, error: FinError }),
  Rpc.make("agent.watch", { payload: AgentWatchPayload, success: WatchItem, error: FinError, stream: true }),
  Rpc.make("agent.approval.list", { payload: AgentApprovalListPayload, success: AgentApprovalListSuccess, error: FinError }),
  Rpc.make("agent.approval.answer", { payload: AgentApprovalAnswerPayload, success: AgentApprovalAnswerSuccess, error: FinError }),
  Rpc.make("agent.approval.wait", { payload: AgentApprovalWaitPayload, success: AgentApprovalWaitSuccess, error: FinError }),
  Rpc.make("agent.providers", { payload: Schema.Struct({}), success: AgentProvidersSuccess, error: FinError }),
  Rpc.make("agent.self", { payload: Schema.Struct({}), success: AgentSelfSuccess, error: FinError }),
  Rpc.make("agent.self.try", { payload: AgentSelfTryPayload, success: AgentSelfSuccess, error: FinError }),
  // Command rules and project trust (7B Task 2); saved prompts (Task 10).
  Rpc.make("agent.rules.list", { payload: AgentRulesListPayload, success: AgentRulesListSuccess, error: FinError }),
  Rpc.make("agent.rules.add", { payload: AgentRulesAddPayload, success: AgentRulesAddSuccess, error: FinError }),
  Rpc.make("agent.rules.remove", { payload: AgentRulesRemovePayload, success: AgentRulesRemoveSuccess, error: FinError }),
  Rpc.make("agent.project.trust", { payload: AgentProjectTrustPayload, success: AgentProjectTrustSuccess, error: FinError }),
  Rpc.make("agent.hooks.list", { payload: AgentHooksListPayload, success: AgentHooksListSuccess, error: FinError }), // 7F
  Rpc.make("agent.savedPrompt.list", { payload: AgentSavedPromptListPayload, success: AgentSavedPromptListSuccess, error: FinError }),
  Rpc.make("agent.savedPrompt.run", { payload: AgentSavedPromptRunPayload, success: AgentSavedPromptRunSuccess, error: FinError }),
  Rpc.make("agent.key.set", { payload: AgentKeySetPayload, success: AgentKeySetSuccess, error: FinError }),
  Rpc.make("agent.key.list", { payload: Schema.Struct({}), success: AgentKeyListSuccess, error: FinError }),
  Rpc.make("agent.key.remove", { payload: AgentKeyRemovePayload, success: AgentKeyRemoveSuccess, error: FinError }),
  Rpc.make("agent.local.set", { payload: AgentLocalSetPayload, success: AgentLocalSetSuccess, error: FinError }),
  Rpc.make("agent.local.show", { payload: Schema.Struct({}), success: AgentLocalShowSuccess, error: FinError }),
  Rpc.make("agent.local.test", { payload: Schema.Struct({}), success: AgentLocalTestSuccess, error: FinError }),
  Rpc.make("agent.local.remove", { payload: AgentLocalRemovePayload, success: AgentLocalRemoveSuccess, error: FinError }),
  Rpc.make("training.closedTargets.get", { payload: Schema.Struct({}), success: TrainingClosedTargetsSuccess, error: FinError }),
  Rpc.make("training.closedTargets.set", { payload: TrainingClosedTargetsSetPayload, success: TrainingClosedTargetsSuccess, error: FinError }),
  ...DEVICE_RPCS, // 5B: pairing, devices, the environment and settings writes (`device-rpcs.ts`)
  ...REVIEW_RPCS, // 5E: the review workflow, its policy and `history.drop` (`review-rpcs.ts`)
);
