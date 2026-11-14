// The retry policy and isolation in the engine (spec §6.5, 5C-1 plan Decisions 31, 32): after
// each attempt the failure is classified and the job retried (at once, or after its backoff,
// `nextRunAt` persisted), isolated, or failed; a batch's runs during isolation start at once
// and spend no attempt. What the policy must remember goes with the row (`jobs.isolation`):
// the isolation state, the results of the runs that succeeded, and whether the one retry on a
// fresh process was used. A batch kind's final result is `combine(results, items)`.
import type { FinError, JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { finError, toFinError } from "../errors.ts";
import { data, type Engine } from "./engine.ts";
import { DEFAULT_HOOK_TIMEOUT_S, runHook } from "./hooks.ts";
import { afterRun, backoffMs, classify, type IsolationState, nextRun, nextStep, startIsolation, withoutDone } from "./isolation.ts";
import { crashRestartable, type ItemOutcome, type KindSpec } from "./kinds.ts";
import type { AttemptOutcome } from "./runner.ts";
import { type JobRow, viewOf } from "./store.ts";

/** What `jobs.isolation` holds. */
export interface RetryMemo {
  /** The one retry at once on a fresh process (`fresh`, `oomWarm`) was used. */
  readonly fresh?: boolean;
  readonly isolation?: IsolationState;
  /** The values of the isolation runs that succeeded, in order. */
  readonly results?: ReadonlyArray<unknown>;
}

export function memoOf(row: JobRow): RetryMemo {
  const m = row.isolation;
  return m !== null && typeof m === "object" ? (m as RetryMemo) : {};
}

const isEmpty = (m: RetryMemo): boolean => m.fresh !== true && m.isolation === undefined && m.results === undefined;

/** The default `combine` (Decision 32): `{items: {<item>: value}, itemErrors}`. */
export function defaultCombine(items: ReadonlyMap<string, ItemOutcome>): unknown {
  const values: Record<string, unknown> = {};
  const itemErrors: Array<{ item: string; error: ReturnType<typeof data> }> = [];
  for (const [item, o] of items) {
    if ("error" in o) itemErrors.push({ item, error: data(o.error) });
    else values[item] = o.value ?? null;
  }
  return { items: values, itemErrors };
}

/** A batch kind's items not yet acknowledged in `job_items`. */
export function itemsLeft(e: Engine, row: JobRow, kind: KindSpec): ReadonlyArray<string> | undefined {
  if (kind.batch === undefined) return undefined;
  const done = e.d.store.items(row.id);
  return kind.batch.items(row.params as never).filter((i) => !done.has(i));
}

/**
 * What the next run of `row` runs: during isolation the head of its queue (acknowledged items
 * dropped); else a batch kind's params restricted to the items not acknowledged yet
 * (acknowledged items never re-run); else the job's params. `complete`: isolation has nothing
 * left to run (its last items were acknowledged before a restart).
 */
export function runPlan(e: Engine, row: JobRow, kind: KindSpec): { readonly params: unknown; readonly items?: ReadonlyArray<string> } | { readonly complete: true } {
  const batch = kind.batch;
  if (batch === undefined) return { params: row.params };
  const iso = memoOf(row).isolation;
  if (iso !== undefined) {
    const next = nextRun(withoutDone(iso, new Set(e.d.store.items(row.id).keys())));
    return next === undefined ? { complete: true } : { params: batch.restrict(row.params as never, next), items: next };
  }
  const all = batch.items(row.params as never);
  const left = itemsLeft(e, row, kind) ?? all;
  return { params: left.length === all.length ? row.params : batch.restrict(row.params as never, left), items: left };
}

/** Back to `queued`: after `delayMs` (`nextRunAt`), or at once; `spent: false` keeps the attempt count. */
function requeue(e: Engine, row: JobRow, kind: KindSpec, o: { error: FinError | null; delayMs: number; memo: RetryMemo; spent: boolean }): void {
  const view = e.transition(row.id, "queued", {
    error: o.error === null ? null : data(o.error),
    nextRunAt: o.delayMs > 0 ? e.d.now() + o.delayMs : null,
    isolation: isEmpty(o.memo) ? null : o.memo,
    ...(o.spent ? {} : { attempts: Math.max(0, row.attempts - 1) }),
  });
  e.dropProgress(row.id);
  e.publish({ type: "state", job: view });
  e.enqueue(view, kind);
  e.pump();
}

/** The kind's `resourceHealthy` (false when it fails or times out, so nothing is poisoned on a doubt). */
async function healthy(e: Engine, row: JobRow, kind: KindSpec): Promise<boolean> {
  const ask = kind.resourceHealthy;
  if (ask === undefined) return true;
  const out = await runHook(e.env, "resourceHealthy", Effect.suspend(() => ask(viewOf(row) as JobView & { params: never })), kind.hookTimeoutS ?? DEFAULT_HOOK_TIMEOUT_S);
  return out.ok && out.value;
}

/** Ends a batch job with `combine(results, items)` (the kind's, or the default). */
async function finish(e: Engine, row: JobRow, kind: KindSpec, results: ReadonlyArray<unknown>): Promise<void> {
  const items = e.d.store.items(row.id);
  let value: unknown;
  try {
    value = kind.batch?.combine === undefined ? defaultCombine(items) : kind.batch.combine(results, items);
  } catch (err) {
    await e.end(row.id, "failed", { error: data(finError("INTERNAL", `the kind's combine failed: ${toFinError(err).message}`)) });
    return;
  }
  await e.succeed(row.id, kind, value);
}

/** Isolation goes on with `state` (its next run at once, no attempt spent), or ends with the result. */
async function continueIsolation(e: Engine, row: JobRow, kind: KindSpec, memo: RetryMemo, error: FinError | null): Promise<void> {
  const state = memo.isolation;
  if (state === undefined || nextRun(state) === undefined) return finish(e, row, kind, memo.results ?? []);
  requeue(e, row, kind, { error, delayMs: 0, memo, spent: false });
}

/** The worker succeeded. A batch kind's result is combined over its runs; during isolation the next run follows. */
export async function afterSuccess(e: Engine, row: JobRow, kind: KindSpec, outcome: AttemptOutcome & { ok: true }, items: ReadonlyArray<string>): Promise<void> {
  if (kind.batch === undefined) return e.succeed(row.id, kind, outcome.value);
  const cur = e.d.store.row(row.id) ?? row;
  const memo = memoOf(cur);
  const results = [...(memo.results ?? []), outcome.value];
  if (memo.isolation === undefined) return finish(e, cur, kind, results);
  const next = afterRun(memo.isolation, { items, done: outcome.done, inFlight: outcome.inFlight, crashed: false });
  if (next === "exhausted") return finish(e, cur, kind, results); // never: a success crashes nothing
  return continueIsolation(e, cur, kind, { ...memo, isolation: next, results }, null);
}

/** Poisoned items become item errors (`POISON_ITEM`), written at once. */
function poison(e: Engine, row: JobRow, items: ReadonlyArray<string>, cause: FinError): void {
  if (items.length === 0) return;
  const outcomes = items.map((item): [string, ItemOutcome] => [
    item,
    { error: finError("POISON_ITEM", `the item ${item} crashes the worker`, { item, cause: cause.code }, "check the item itself") },
  ]);
  e.d.store.writeItems(row.id, outcomes);
  for (const [item, outcome] of outcomes) e.env.item(row.id, item, outcome);
}

/** A failed attempt (not taken over by a stop): classified, then retried, isolated or failed (Decision 31). */
export async function afterFailure(e: Engine, row: JobRow, kind: KindSpec, outcome: AttemptOutcome & { ok: false }, items: ReadonlyArray<string>): Promise<void> {
  const cur = e.d.store.row(row.id) ?? row;
  const memo = memoOf(cur);
  const facts = {
    cause: outcome.cause,
    error: outcome.error,
    ...(outcome.retry === undefined ? {} : { retry: outcome.retry }),
    ...(outcome.exit === undefined ? {} : { exit: outcome.exit }),
    ...(outcome.warmReused === true ? { warmReused: true } : {}),
  };
  let c = classify(facts);
  const interactive = row.priority === "interactive";
  // Only cold batch jobs are isolated: a warm job never is (Decision 31).
  const batch = kind.batch !== undefined && !interactive && cur.warm !== true;
  const ctx = { attempts: row.attempts, maxAttempts: row.maxAttempts, interactive, batch, inFlight: outcome.inFlight.length, freshUsed: memo.fresh === true };
  // `crashRestarts` (6C Task 18): a crashed one-item run restarts from zero while attempts remain;
  // a crash after its item was acknowledged ends the job with it (its output was complete).
  if (batch && c.cls === "crash" && crashRestartable(kind, cur.params) && memo.isolation === undefined) {
    if (e.crashed) return;
    if ((itemsLeft(e, cur, kind) ?? []).length === 0) return finish(e, cur, kind, memo.results ?? []);
    if (row.attempts < row.maxAttempts) {
      requeue(e, cur, kind, { error: c.error, delayMs: backoffMs(row.attempts), memo, spent: true });
      return;
    }
  }
  let step = nextStep(c.cls, ctx);
  // Asked before any POISON_ITEM verdict: an unhealthy resource makes the crash a resource fault.
  if (step.do === "isolate" && !(await healthy(e, cur, kind))) {
    c = classify({ ...facts, healthy: false });
    step = nextStep(c.cls, ctx);
  }
  if (e.crashed) return;
  switch (step.do) {
    case "fail":
      await e.end(row.id, "failed", { error: data(c.error) });
      return;
    case "retry":
      requeue(e, cur, kind, { error: c.error, delayMs: step.delayMs, memo: { ...memo, ...(c.cls === "fresh" || c.cls === "oomWarm" ? { fresh: true } : {}) }, spent: true });
      return;
    case "isolate": {
      const state = memo.isolation ?? startIsolation(items, kind.batch?.items(row.params as never).length ?? items.length);
      const next = afterRun(state, { items, done: outcome.done, inFlight: outcome.inFlight, crashed: true });
      e.markIsolated(row.id);
      if (next === "exhausted") {
        const error = finError("WORKER_CRASHED", "the batch kept crashing and no single item explains it", {
          ...(c.error.details as Record<string, unknown>),
          isolation: "budget",
          crashRuns: state.crashRuns + 1,
          poisoned: state.poisoned.length,
        });
        await e.end(row.id, "failed", { error: data(error) });
        return;
      }
      poison(e, cur, next.poisoned.slice(state.poisoned.length), c.error);
      await continueIsolation(e, cur, kind, { ...memo, isolation: next }, c.error);
      return;
    }
  }
}

/** A dispatched job whose isolation has nothing left to run (its items were acknowledged before a restart). */
export async function completeIsolation(e: Engine, row: JobRow, kind: KindSpec): Promise<void> {
  await finish(e, row, kind, memoOf(row).results ?? []);
}
