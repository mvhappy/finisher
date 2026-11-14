// A dispatched job's run (spec §6.0–6.5, 5C-1 plan Tasks 9a, 9b, 11): its attempt, cold or on a
// lent warm worker, then what follows it (success and the hooks, cancel, pause or close, the
// retry policy). Whatever throws, the slots go back and the job ends (9a review H3). Split out
// of `engine.ts` (Task 12).
import { finError, toFinError } from "../errors.ts";
import { pausedAfterRun } from "./control.ts";
import { data, type Engine, finished } from "./engine.ts";
import type { FamilySpec, KindSpec } from "./kinds.ts";
import { interruptRow } from "./restart.ts";
import { afterFailure, afterSuccess, completeIsolation, runPlan } from "./retry.ts";
import type { RunControl } from "./run-control.ts";
import { type AttemptOutcome, runAttempt } from "./runner.ts";
import type { JobRow } from "./store.ts";

/** A warm worker lent to one dispatch (`warm.ts`): `run` is its attempt; `release` gives it back unused. */
export interface WarmLease {
  run(row: JobRow, kind: KindSpec, family: FamilySpec, params: unknown, control: RunControl): Promise<AttemptOutcome>;
  release(): void;
}

/**
 * Whether a run stopped by a pause or close ended because of the stop: stopped before the
 * worker started, or after the stop reached it without its own answer (an exit, a helper
 * killed with the group, or the worker's `JOB_CANCELLED`).
 */
export function stoppedRun(o: AttemptOutcome): boolean {
  if (o.ok) return false;
  if (o.cause === "stopped") return true;
  return o.stopSent && (o.cause === "exit" || o.cause === "helper" || (o.cause === "worker" && o.error.code === "JOB_CANCELLED"));
}

/** One attempt of dispatched job `id` and what follows it. */
export async function runDispatched(e: Engine, id: string, resources: ReadonlyArray<string>, control: RunControl, lease?: WarmLease): Promise<void> {
  let given = false;
  const give = (): void => {
    if (given) return;
    given = true;
    e.d.pools.give(resources);
    // The warm worker is ready again (or gone): what waits for it or for the slots goes now.
    if (lease !== undefined) {
      lease.release();
      e.pump();
    }
  };
  try {
    const row = e.d.store.row(id);
    const kind = row === undefined ? undefined : e.kinds.kind(row.kind);
    const family = kind === undefined ? undefined : e.kinds.family(kind.family);
    if (row === undefined || kind === undefined || family === undefined) {
      give();
      if (row !== undefined) await e.end(id, "failed", { error: data(finError("JOB_KIND_UNKNOWN", `no kind ${row.kind}`, { kind: row.kind })) });
      return;
    }
    const plan = runPlan(e, row, kind);
    if ("complete" in plan) {
      give();
      await completeIsolation(e, row, kind);
      return;
    }
    const outcome = lease === undefined ? await runAttempt(e.env, row, kind, family, plan.params, control) : await lease.run(row, kind, family, plan.params, control);
    const stats = e.statsOf(id);
    stats.runMicros += outcome.runMicros;
    stats.helpers += outcome.helpers;
    give();
    if (e.crashed) return;
    await afterAttempt(e, id, row, kind, outcome, control, plan.items ?? []);
  } catch (err) {
    give();
    if (e.crashed) return;
    e.d.log("error", `job ${id}: the runner failed: ${(err as Error).stack ?? String(err)}`);
    const row = e.d.store.row(id);
    if (row !== undefined && !finished(row)) {
      await e.end(id, "failed", { error: data(finError("INTERNAL", `the job's runner failed: ${toFinError(err).message}`)) });
    }
  } finally {
    e.runs.delete(id);
    e.wakeWaiters(id);
  }
}

async function afterAttempt(e: Engine, id: string, row: JobRow, kind: KindSpec, outcome: AttemptOutcome, control: RunControl, items: ReadonlyArray<string>): Promise<void> {
  if (outcome.ok) {
    await afterSuccess(e, row, kind, outcome, items);
    return keepPause(e, id, control);
  }
  if (control.requested === "cancel") {
    await e.end(id, "cancelled", { error: data(finError("JOB_CANCELLED", "the job was cancelled")) });
    return;
  }
  // A pause or close takes over only a run it stopped; a definitive outcome (the worker's own
  // error, a crash before the stop reached it) follows the policy (9b review M1).
  if (control.requested !== undefined && stoppedRun(outcome)) {
    if (control.pauseRequested) pausedAfterRun(e, id); // a pause asked before a close holds (9b review M2)
    else await interruptRow(e, e.d.store.row(id) ?? row, kind, true);
    return;
  }
  await afterFailure(e, row, kind, outcome, items);
  keepPause(e, id, control);
}

/** A pause asked during a run the policy re-queued (a retry, the next isolation run) still pauses the job. */
function keepPause(e: Engine, id: string, control: RunControl): void {
  if (!control.pauseRequested || control.requested === "cancel" || e.closing || e.d.store.row(id)?.state !== "queued") return;
  e.queue.remove(id);
  e.held.get(id)?.();
  e.held.delete(id);
  e.publish({ type: "state", job: e.transition(id, "paused", { pausedFrom: "queued" }) });
}
