import type { CommittedTx, EngineEvent, Probe, Project, Tx, TxSetAside } from "@finisher/core-types";
import { Schema } from "effect";
import { Rpc, RpcGroup } from "effect/rpc";
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

/**
 * Envelope types generated from the Rust types they carry (`@finisher/core-types`). The Effect
 * schemas below that validate them at runtime are checked against the same generated schema
 * (`envelopes.test.ts`).
 */
export type { EngineEvent, Generation, Probe, TxSetAside } from "@finisher/core-types";

/** Narrowed views of the `Schema.Unknown` payloads; runtime validation is Rust's job. */
export type TxPayload = Tx;
export type ProjectPayload = Project;
export type CommittedTxPayload = CommittedTx;

export const ServerHelloPayload = Schema.Struct({});
export const ServerHelloSuccess = Schema.Struct({
  protocolVersion: Schema.Number,
  serverVersion: Schema.String,
  capabilities: Schema.Array(Schema.String),
});

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
});
export const ProjectRefSuccess = Schema.Struct({
  projectId: Schema.String,
  rev: Schema.Number,
});

export const AssetProbePayload = Schema.Struct({ path: Schema.String });
/** What `asset.probe` answers: fin-core's `Probe` (the generated type; `envelopes.test.ts` checks the fields). */
export const ProbeSchema = Schema.Struct({
  audio: Schema.optionalKey(Schema.NullOr(Schema.Struct({ channels: Schema.Number, sampleRate: Schema.Number }))),
  duration: Schema.String,
  fingerprint: Schema.String,
  size: Schema.Number,
  video: Schema.optionalKey(
    Schema.NullOr(Schema.Struct({ height: Schema.Number, rate: Schema.String, width: Schema.Number })),
  ),
});
export const AssetProbeSuccess = ProbeSchema;
// The schema's type and the generated `Probe` must stay assignable to each other.
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;
export const probeMatchesGenerated: Same<typeof ProbeSchema.Type, Probe> = true;

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
 * rebuilt since, instead of being rebased over a history the client never saw. Optional so
 * that protocol-1 clients keep working; every client that reads before it writes should send
 * it (`fin` always does).
 */
export const TxCommitPayload = Schema.Struct({
  projectId: Schema.String,
  tx: Schema.Unknown,
  generation: Schema.optional(GenerationSchema),
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
/** fin-store's `EngineEvent` less `applied` (fed to `ops.subscribe`) and `lagged` (ends subscribers). */
export type ProjectEvent = Exclude<EngineEvent, { type: "applied" } | { type: "lagged" }>;

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
 * instead of continuing a history the caller never saw. Optional for protocol-1 clients.
 */
export const OpsSubscribePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  fromRev: Schema.Number,
  generation: Schema.optional(GenerationSchema),
});

export const FinRpcs = RpcGroup.make(
  Rpc.make("server.hello", {
    payload: ServerHelloPayload,
    success: ServerHelloSuccess,
    error: FinError,
  }),
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
);
