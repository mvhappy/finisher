// Start-up recovery, retention and close (spec §6.5, 5C-1 plan Decisions 28–30). At start:
// leftover directories are swept (but those of `phase: hooks` jobs), `job.done` goes out for
// terminal rows whose announcement was cut short (9a review H2), `phase: hooks` rows re-run
// their `onSucceeded` with `rerun: true`, other `running` rows are interrupted and the
// resumable ones with attempts left go back to the queue; `paused` rows stay paused. Retention
// runs at start and hourly. `close` pauses running jobs and recovers them at once, so a clean
// stop leaves no `running` row in `phase: run`.
import { Fiber } from "effect";
import { finError, toFinError } from "../errors.ts";
import { data, type Engine } from "./engine.ts";
import { sweep } from "./files.ts";
import type { KindSpec } from "./kinds.ts";
import type { JobRow, JobStore } from "./store.ts";
import { viewOf } from "./store.ts";

export const RETENTION_EVERY_MS = 60 * 60_000;
/** How long `close` gives a running job to checkpoint before its group is killed. */
export const CLOSE_GRACE_MS = 2000;
const SHUTDOWN_GRACE_MS = 2000;
const CRASH_WAIT_MS = 5000;

/** A batch kind's items not yet acknowledged in `job_items`. */
function itemsLeft(store: JobStore, row: JobRow, kind: KindSpec): ReadonlyArray<string> | undefined {
  if (kind.batch === undefined) return undefined;
  const done = store.items(row.id);
  return kind.batch.items(row.params as never).filter((i) => !done.has(i));
}

/** A `resumable` kind, or a batch kind with items left (Decision 28). */
export function isResumable(store: JobStore, row: JobRow, kind: KindSpec): boolean {
  if (kind.resumable === true) return true;
  const left = itemsLeft(store, row, kind);
  return left !== undefined && left.length > 0;
}

/**
 * What the next attempt runs: a batch kind's params restricted to the items not acknowledged
 * yet (acknowledged items never re-run); otherwise the job's params. Task 10 adds isolation.
 */
export function runParams(store: JobStore, row: JobRow, kind: KindSpec): unknown {
  const left = itemsLeft(store, row, kind);
  if (left === undefined || kind.batch === undefined || left.length === kind.batch.items(row.params as never).length) return row.params;
  return kind.batch.restrict(row.params as never, left);
}

/**
 * A `running` row in `phase: run` whose run is over without an outcome (the last run died, or
 * `close` stopped it): `interrupted`; back to `queued` when resumable with attempts left (the
 * interrupted run counted as one), else it stays `interrupted` with `JOB_INTERRUPTED`.
 */
export async function interruptRow(e: Engine, row: JobRow, kind: KindSpec | undefined): Promise<void> {
  const error = data(finError("JOB_INTERRUPTED", "the job was interrupted by a server stop", {}, "submit it again"));
  if (kind !== undefined && row.attempts < row.maxAttempts && isResumable(e.d.store, row, kind)) {
    e.transition(row.id, "interrupted", { error });
    const view = e.transition(row.id, "queued", { error: null });
    e.dropProgress(row.id);
    e.publish({ type: "state", job: view });
    if (!e.closing) e.enqueue(view, kind);
    return;
  }
  await e.end(row.id, "interrupted", { error });
}

/** Start-up recovery (Decision 28) and the first retention pass. */
export async function recover(e: Engine): Promise<void> {
  const { store, root, log } = e.d;
  const keep = new Set(store.inHooks().map((j) => j.id));
  const removed = await sweep(root, keep);
  if (removed.length > 0) log("debug", `job engine: swept ${removed.length} leftover job directories`);
  for (const row of store.unannounced()) e.announce(viewOf(row));
  for (const row of store.rowsIn("running")) {
    const kind = e.kinds.kind(row.kind);
    if (row.phase === "hooks") {
      if (kind === undefined) {
        await e.end(row.id, "failed", { error: data(finError("JOB_KIND_UNKNOWN", `no kind ${row.kind}`, { kind: row.kind })) });
      } else {
        log("warning", `job ${row.id}: the last run stopped in its onSucceeded hook; it runs again`);
        e.track(e.runHooks(row, kind, row.result, true));
      }
      continue;
    }
    await interruptRow(e, row, kind);
  }
  for (const j of store.queued()) {
    const kind = e.kinds.kind(j.kind);
    if (kind === undefined) {
      log("warning", `job ${j.id}: its kind ${j.kind} is not registered; it stays queued`);
      continue;
    }
    e.enqueue(j, kind);
  }
  retention(e);
}

/** Decision 30: now, then every hour on the engine's clock. */
function retention(e: Engine): void {
  if (e.closing) return;
  e.d.run(e.d.store.retention(e.d.now())).addObserver((exit) => {
    if (exit._tag === "Failure") e.d.log("warning", `job retention failed: ${String(exit.cause)}`);
  });
  e.stopRetention = e.d.timers.after(RETENTION_EVERY_MS, () => retention(e));
}

function stopTimers(e: Engine): void {
  e.closing = true;
  e.stopRetention?.();
  e.cancelWake();
}

/**
 * Graceful close (Decision 29): dispatch stops; jobs in their hook phase finish it (bounded by
 * `hookTimeoutS`); running jobs get `cancel {reason: pause}` and 2 s, then their groups are
 * killed and they are recovered as at a start; then the host shuts down.
 */
export async function closeEngine(e: Engine): Promise<void> {
  if (e.closing) return;
  stopTimers(e);
  for (const [id, control] of e.runs) {
    const row = e.d.store.row(id);
    if (row?.phase === "run") control.request("close", CLOSE_GRACE_MS);
  }
  while (e.active.size > 0) await Promise.allSettled([...e.active]);
  e.dropAllProgress();
  await e.d.host.shutdown(SHUTDOWN_GRACE_MS).catch((err) => e.d.log("warning", `job host shutdown: ${toFinError(err).message}`));
  e.shutdownBus();
}

/** A simulated crash (tests): nothing more is written; the host goes at once. */
export async function crashEngine(e: Engine): Promise<void> {
  if (e.crashed) return;
  e.crashed = true;
  stopTimers(e);
  for (const f of e.env.hooks) e.d.run(Fiber.interrupt(f));
  await e.d.host.shutdown(0).catch(() => {});
  // Real time: a test's TestClock may stand still.
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([Promise.allSettled([...e.active]), new Promise<void>((r) => (timer = setTimeout(r, CRASH_WAIT_MS)))]);
  clearTimeout(timer);
  e.dropAllProgress();
  e.shutdownBus();
}
