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
export const ProjectOpenPayload = Schema.Struct({ dir: Schema.String });
export const ProjectRefSuccess = Schema.Struct({
  projectId: Schema.String,
  rev: Schema.Number,
});

export const AssetProbePayload = Schema.Struct({ path: Schema.String });
export const AssetProbeSuccess = Schema.Unknown;

export const TxCommitPayload = Schema.Struct({
  projectId: Schema.String,
  tx: Schema.Unknown,
});
export const CommittedTxSchema = Schema.Unknown;

export const SnapshotGetPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
});
export const SnapshotGetSuccess = Schema.Struct({
  rev: Schema.Number,
  project: Schema.Unknown,
});

export const OpsSubscribePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  fromRev: Schema.Number,
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
);
