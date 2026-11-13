// Branch and history calls (plan 4B Task 18): which engine call serves each RPC, how a payload
// becomes the engine's request JSON, and which records of a result are published to which hub.
import type { CommittedTx } from "@finisher/core-types";
import { blake3Hex, type Engine } from "@finisher/fin-node";
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
  | "drop"
  | "recover"
  // A read (5E Decision 7): the change list a merge would make now. Under the commit lock like
  // the writes, so a review never plans against a write half made; it publishes nothing.
  | "reviewPlan";

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

/** A call's actor (`actorFor(principal, headers)`) as the engine's `metaJson`: its `log.commit` actor. */
export function metaJson(meta: { readonly actor: object } | undefined): string | undefined {
  return meta === undefined ? undefined : JSON.stringify(meta);
}

/**
 * `history.drop` always carries an `argsHash` (5E Task 2 review): a client's own (`fin` sends one
 * for every write), else one made here from the request's fields, so a retry of a `txId` with
 * other arguments is `IDEM_KEY_REUSED`, never the first record's answer. The origin lists are
 * sorted first: the order they were named in is not part of the request.
 *
 * The hash made here is not `fin`'s: `fin drop` hashes its own command input (`args_hash` over
 * `[{cmd: "drop", args: {from, into, drops|undrops}}]`, `into` as given or the current branch, only the list it sends),
 * this one `{cmd: "drop", args: {from, into, drops, undrops}}` with both lists. They are never
 * compared with each other: each is checked only against a retry of the same txId, which comes
 * from the same client and so carries the same form. A txId `fin` used and a raw client retries
 * without a hash is `IDEM_KEY_REUSED`, never a second record (Task 5 review, N3).
 */
export function dropPayload<P extends { readonly argsHash?: string | undefined }>(
  payload: P & { readonly from: string; readonly into: string; readonly drops?: ReadonlyArray<string> | undefined; readonly undrops?: ReadonlyArray<string> | undefined },
): P {
  if (payload.argsHash !== undefined) return payload;
  const sorted = (l: ReadonlyArray<string> | undefined) => [...new Set(l ?? [])].sort();
  const args = { cmd: "drop", args: { from: payload.from, into: payload.into, drops: sorted(payload.drops), undrops: sorted(payload.undrops) } };
  return { ...payload, argsHash: `blake3:${blake3Hex(Buffer.from(JSON.stringify(args)))}` };
}

export function callEngine(engine: Engine, op: HistoryOp, payload: object, meta?: string): Promise<string> {
  if (op === "reviewPlan") return engine.reviewPlan(requestJson(payload));
  return engine[op](requestJson(payload), meta);
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
