import type {
  AudioPlan,
  CommittedTx,
  EngineEvent,
  FontCatalog,
  FontsPlan,
  FontsStatus,
  FramePlan,
  Probe,
  Project,
  Tx,
  TxSetAside,
} from "@finisher/core-types";
import { Schema } from "effect";
import { Rpc, RpcGroup } from "effect/rpc";
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
  ServerHelloPayload,
  ServerHelloSuccess,
  ServerStatusSuccess,
  ServerStopSuccess,
} from "./auth.ts";
import { FinError } from "./errors.ts";
import {
  BranchCreatePayload,
  BranchCreateSuccess,
  BranchListPayload,
  BranchListSuccess,
  BranchSetPayload,
  HistoryCheckpointPayload,
  HistoryCherryPickPayload,
  HistoryMergePayload,
  HistoryRedoPayload,
  HistoryResultSuccess,
  HistoryRevertPayload,
  HistoryUndoPayload,
  MergeReportSuccess,
  ProjectRecoverPayload,
  RecordSuccess,
  RecoverReportSuccess,
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

/** Narrowed views of the `Schema.Unknown` payloads; runtime validation is Rust's job. */
export type TxPayload = Tx;
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
  /** The console's typed confirmation of a relocation (5A Decision 30). */
  challenge: ChallengeField,
});
export const ProjectRefSuccess = Schema.Struct({
  projectId: Schema.String,
  rev: Schema.Number,
});

/** Protocol 2: the probe is for `projectId` (reach: the path must be in that project's folder). */
export const AssetProbePayload = Schema.Struct({ projectId: Schema.String, path: Schema.String });
const Optional = <S extends Schema.Top>(s: S) => Schema.optionalKey(Schema.NullOr(s));
const ImageInfoSchema = Schema.Struct({ height: Schema.Number, width: Schema.Number });
const LottieInfoSchema = Schema.Struct({
  duration: Schema.String,
  fonts: Schema.optionalKey(Schema.mutable(Schema.Array(Schema.String))),
  height: Schema.Number,
  rate: Schema.String,
  slots: Schema.optionalKey(
    Schema.mutable(Schema.Array(Schema.Struct({ id: Schema.String, kind: Schema.String }))),
  ),
  width: Schema.Number,
});
const FontFileSchema = Schema.Struct({
  faces: Schema.mutable(
    Schema.Array(
      Schema.Struct({
        family: Schema.String,
        index: Schema.Number,
        italic: Schema.Boolean,
        scripts: Schema.mutable(Schema.Array(Schema.String)),
        weight: Schema.Number,
      }),
    ),
  ),
  format: Schema.Literals(["ttf", "otf", "ttc"]),
  sha256: Schema.String,
  size: Schema.Number,
});
/**
 * What `asset.probe` answers: fin-probe's `Probe` (the generated type; `envelopes.test.ts`
 * checks the fields): the asset kind its bytes show and that kind's fields, absent ones omitted.
 */
export const ProbeSchema = Schema.Struct({
  audio: Optional(Schema.Struct({ channels: Schema.Number, sampleRate: Schema.Number })),
  duration: Optional(Schema.String),
  fingerprint: Schema.String,
  font: Optional(FontFileSchema),
  image: Optional(ImageInfoSchema),
  kind: Schema.Literals(["media", "image", "svg", "lottieTemplate", "font", "lut"]),
  lottie: Optional(LottieInfoSchema),
  lut: Optional(Schema.Struct({ size: Schema.Number })),
  // MP4/MOV only: fin-media's `MediaInfo` (the generated type), what the file is (display size,
  // rotation, colour, HDR, VFR). Validated in Rust and passed through; never in `asset.add`.
  media: Optional(Schema.Unknown),
  size: Schema.Number,
  video: Optional(Schema.Struct({ height: Schema.Number, rate: Schema.String, width: Schema.Number })),
});
export const AssetProbeSuccess = ProbeSchema;
// The schema's type and the generated `Probe` must stay assignable to each other (`media` is a
// pass-through, typed by the generated `MediaInfo`).
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;
export const probeMatchesGenerated: Same<Omit<typeof ProbeSchema.Type, "media">, Omit<Probe, "media">> = true;

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
export const CommittedTxSchema = Schema.Unknown;

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
/** fin-store's `ProjectStatus` (camelCase) plus `busy: null | { operation, sinceMs }`. */
export const ProjectStatusSchema = Schema.Unknown;
export const ProjectTakeoverPayload = Schema.Struct({
  projectId: Schema.String,
  /** Needed on sync folders, which never take over without a person's say-so. */
  confirm: Schema.Boolean,
  /** The console's typed confirmation of `confirm: true` (5A Decision 30). */
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

export const TxGetPayload = Schema.Struct({ projectId: Schema.String, txId: Schema.String });
/**
 * A tx that is not (or no longer) on its branch: kept in quarantine. `reason`: `SUPERSEDED`,
 * `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`, `MALFORMED`,
 * `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`.
 */
export const TxSetAsideSchema = Schema.Struct({
  setAside: Schema.Struct({
    txId: Schema.String,
    branch: Schema.String,
    reason: Schema.String,
    origin: Schema.String,
  }),
});
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

/**
 * Plan 4C Decisions 40–43: evaluation runs on the server. `branch` defaults to `main` (an id,
 * a name or `<name>#<id>`), `timeline` to the main (first) timeline; times are rational
 * strings (`"12"`, `"1001/30000"`). Captions are measured with the server's shared measurer
 * from the project's pinned font bytes; a caption whose bytes are missing is unmeasured
 * (`layout: null`, `unmeasured`), never an error.
 */
export const EvalFramePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  at: Schema.String,
});
export const EvalAudioPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  from: Schema.String,
  to: Schema.String,
});
/**
 * Plan 6A Task 13: frames of a timeline as PNGs, rendered by a `media.look` job on the media
 * worker. `at`: 1 to 64 rational times (`"12"`, `"1001/30000"`, canonical; past the timeline's
 * end is `INVALID_RANGE`). `branch` defaults to `main`, `timeline` to the first. `out`: an
 * absolute folder for the files (`workspace/shared/look/` of the project when absent; never
 * inside `.finisher/`, `assets/` or the server's data, `EXPORT_DESTINATION`); naming it needs
 * `project.write` on top of `project.read` and `jobs.submit`, and a principal with roots writes
 * only inside its project's folder. `size`: each side 1 to 8192 (the timeline's resolution when
 * absent). `wait` (default true) answers the `LookResult` (frames with their final paths, what
 * was `skipped`, the adapter) once the job finished, or its error; `wait: false` answers
 * `{job}` at once.
 */
export const MediaLookPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  at: Schema.Array(Schema.String),
  out: Schema.optional(Schema.String),
  size: Schema.optional(Schema.Struct({ width: Schema.Number, height: Schema.Number })),
  wait: Schema.optional(Schema.Boolean),
});
/** The generated `LookResult` (`wait`), or `{job}` (the job's view: `wait: false`). */
export const MediaLookSuccess = Schema.Unknown;
/** The generated `MediaCapabilities`: `{codecs, gpu, worker}`. */
export const MediaCapabilitiesSuccess = Schema.Unknown;
/** fin-core's `FramePlan` / `AudioPlan` (the generated types). */
export const FramePlanSchema = Schema.Unknown;
export const AudioPlanSchema = Schema.Unknown;
export type FramePlanPayload = FramePlan;
export type AudioPlanPayload = AudioPlan;

/** fin-fonts' `FontCatalog`: the catalog this server fetches from. */
export const FontCatalogSchema = Schema.Unknown;
export type FontCatalogPayload = FontCatalog;
/**
 * fin-fonts' `FontsStatus`: every font file's state in this server (`cached | missing |
 * queued | fetching {bytes, total} | failed {reason, retryAtMs}`), `autoFetch` and
 * `damaged`. A project's own font summary is in `project.status` (`fonts {missing, fetching,
 * failed}`, warnings `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONTS_DAMAGED`).
 */
export const FontStatusSchema = Schema.Unknown;
export type FontStatusPayload = FontsStatus;
/**
 * `fonts.fetch`: fetch now and wait (background fetching being off does not stop it).
 * `sha256`: catalog files; `entries`: catalog entry ids (each of their styles); `keys`: font
 * keys `<family>|<weight>|<normal|italic>` (the catalog's face); `pinned`: a project id
 * (what its branches pin). Fails with `FONT_FETCH_FAILED` (`details {key, entry, style,
 * sha256, reason, retryAt}`), `FONT_NOT_IN_CATALOG`, `PROJECT_NOT_FOUND`, `FONTS_DAMAGED`.
 */
export const FontsFetchPayload = Schema.Struct({
  sha256: Schema.optional(Schema.Array(Schema.String)),
  entries: Schema.optional(Schema.Array(Schema.String)),
  keys: Schema.optional(Schema.Array(Schema.String)),
  pinned: Schema.optional(Schema.String),
});
/**
 * `fonts.plan` (Decision 35): the `font.pin` ops a client prepends to its own tx of `ops`
 * before committing it; `notes` list faces or scripts the catalog cannot pin. The server
 * applies `ops` to a copy of the head (a tx that cannot apply fails here as at commit) and
 * never alters a client's tx. Planned files are fetched.
 */
export const FontsPlanPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  ops: Schema.Array(Schema.Unknown),
});
export const FontsPlanSchema = Schema.Unknown;
export type FontsPlanResult = FontsPlan;

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
  Rpc.make("server.stop", { payload: Schema.Struct({}), success: ServerStopSuccess, error: FinError }),
  Rpc.make("auth.audit.list", { payload: AuditListPayload, success: AuditListSuccess, error: FinError }),
  Rpc.make("auth.audit.purge", { payload: AuditPurgePayload, success: AuditPurgeSuccess, error: FinError }),
  Rpc.make("project.create", {
    payload: ProjectCreatePayload,
    success: ProjectRefSuccess,
    error: FinError,
  }),
  Rpc.make("project.open", {
    payload: ProjectOpenPayload,
    success: ProjectRefSuccess,
    error: FinError,
  }),
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
  Rpc.make("project.verify", { payload: ProjectIdPayload, success: Schema.Unknown, error: FinError }),
  /** fin-store's `RebuildReport`. */
  Rpc.make("project.rebuild", { payload: ProjectIdPayload, success: Schema.Unknown, error: FinError }),
  /** fin-store's `CompactReport` (a forced run). */
  Rpc.make("project.compact", { payload: ProjectIdPayload, success: Schema.Unknown, error: FinError }),
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
);
