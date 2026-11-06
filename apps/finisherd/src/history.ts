// Branch and history calls (plan 4B Task 18): which engine call serves each RPC, how a payload
// becomes the engine's request JSON, and which records of a result are published to which hub.
import type { CommittedTx } from "@finisher/core-types";
import type { Engine } from "@finisher/fin-node";
import type { Hubs } from "./hubs.ts";

/** The engine calls that take a request JSON and write (or, for `recover`, may). */
export type HistoryOp =
  | "branchCreate"
  | "branchSet"
  | "undo"
  | "redo"
  | "checkpoint"
  | "merge"
  | "cherryPick"
  | "revert"
  | "recover";

/** An RPC payload of a history call: the project, `dryRun` and the request's own fields. */
export interface HistoryPayload {
  readonly projectId: string;
  readonly dryRun?: boolean | undefined;
  readonly [field: string]: unknown;
}

/** The engine's request: the RPC payload less the project (the field names are the same). */
export function requestJson(payload: object): string {
  const { projectId: _projectId, ...request } = payload as { projectId?: unknown };
  return JSON.stringify(request);
}

export function callEngine(engine: Engine, op: HistoryOp, payload: object): Promise<string> {
  return engine[op](requestJson(payload));
}

const isRecord = (v: unknown): v is CommittedTx =>
  typeof v === "object" && v !== null && "rev" in v && "tx" in v && "bindings" in v;

/**
 * Every record a result wrote: the bare `CommittedTx` (`branch.set`, `history.checkpoint`), a
 * `BranchCreated`'s `record`, or the `committed` list of a `HistoryResult`, `MergeReport` or
 * `RecoverReport`.
 */
export function committedOf(result: unknown): ReadonlyArray<CommittedTx> {
  if (isRecord(result)) return [result];
  if (typeof result !== "object" || result === null) return [];
  const r = result as { record?: unknown; committed?: unknown };
  if (isRecord(r.record)) return [r.record];
  return Array.isArray(r.committed) ? r.committed.filter(isRecord) : [];
}

/**
 * Publishes a result's records, each to its own branch's hub, in rev order (a recovery writes
 * `branch` records on several parents and copies on the new branches; per branch the order is
 * what matters). A dry run wrote nothing: the result says so (`dryRun`), or, for a bare
 * record, the request did.
 */
export function publishResult(hubs: Hubs, projectId: string, result: unknown, requestedDry: boolean): void {
  const dry = (result as { dryRun?: unknown } | null)?.dryRun;
  if (dry === true || (dry === undefined && requestedDry)) return;
  const records = [...committedOf(result)].sort((a, b) => a.rev - b.rev);
  for (const c of records) hubs.publishTx(projectId, c);
}
