import type { CommittedTx, Project, Tx } from "@finisher/core-types";
import { Schema } from "effect";
import { Rpc, RpcGroup } from "effect/rpc";
import { FinError } from "./errors.ts";

/**
 * TODO(sub-project 4): generate Probe from fin-probe's schema. Until then it is
 * opaque here; Rust validates it.
 */
export type Probe = unknown;

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

export const ProjectCreatePayload = Schema.Struct({
  dir: Schema.String,
  name: Schema.String,
  rate: Schema.String,
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
export const AssetProbeSuccess = Schema.Unknown;

/**
 * What a client read a branch at: the database's `incarnation` (new after every rebuild) and
 * the branch's rewind count `n`. `snapshot.get` and `project.status` (`incarnation` plus
 * `branches[].generation`) report it.
 */
export const GenerationSchema = Schema.Struct({
  incarnation: Schema.String,
  n: Schema.Number,
});
export type Generation = typeof GenerationSchema.Type;

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
/** `{ manual }`: the documented manual procedure (spec §3.7); a real recovery comes in 4B. */
export const ProjectRecoverSuccess = Schema.Struct({ manual: Schema.String });
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
export type ProjectEvent = typeof ProjectEventSchema.Type;

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
export type TxSetAside = typeof TxSetAsideSchema.Type;
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
  Rpc.make("project.recover", { payload: ProjectIdPayload, success: ProjectRecoverSuccess, error: FinError }),
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
