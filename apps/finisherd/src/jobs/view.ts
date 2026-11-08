// Views and bus events of the job engine (spec §6.5, §6.8; 5C-1 plan Decisions 42–43): the
// in-memory progress laid over a stored view, the `events()` items, and `job.done` from a
// terminal view and the job's run statistics.
import type { JobProgress, JobView } from "@finisher/contracts";
import type { JobDoneEvent } from "./events.ts";
import type { KindSpec } from "./kinds.ts";

/** What `events()` carries (5C-2's `jobs.watch` serves it). */
export type JobEvent =
  | { readonly type: "state"; readonly job: JobView }
  | { readonly type: "progress"; readonly job: JobView }
  | { readonly type: "item"; readonly job: JobView; readonly item: string; readonly error?: { readonly code: string } };

/** `view` with its live progress (progress is never stored, Decision 26). */
export function withProgress(view: JobView, progress: JobProgress | undefined): JobView {
  return progress === undefined || view.finishedAt !== undefined ? view : { ...view, progress };
}

/** What the engine counts per job across its attempts (in memory). */
export interface RunStats {
  runMicros: number;
  helpers: number;
  isolated: boolean;
}

export function newStats(): RunStats {
  return { runMicros: 0, helpers: 0, isolated: false };
}

const OUTCOMES = { succeeded: "succeeded", failed: "failed", cancelled: "cancelled", interrupted: "interrupted" } as const;

/**
 * `job.done` for a terminal view (Decision 42): the kind's `journal(job)` fields as `extra`,
 * a batch kind's item count and failed items. A throwing `journal` loses only its fields.
 */
export function doneEvent(view: JobView, kind: KindSpec | undefined, stats: RunStats, log: (m: string) => void): JobDoneEvent {
  const outcome = OUTCOMES[view.state as keyof typeof OUTCOMES];
  if (outcome === undefined) throw new Error(`job ${view.id} is not finished (${view.state})`);
  const queuedMs = Math.max(0, (view.startedAt ?? view.finishedAt ?? view.createdAt) - view.createdAt);
  let items: { items?: number; itemsFailed?: number } = {};
  if (kind?.batch !== undefined) {
    try {
      items = { items: kind.batch.items(view.params as never).length };
    } catch (err) {
      log(`job ${view.id}: batch.items threw: ${String(err)}`);
    }
    items = { ...items, itemsFailed: view.itemErrors?.length ?? 0 };
  }
  let extra: { extra?: Readonly<Record<string, unknown>> } = {};
  if (kind?.journal !== undefined) {
    try {
      extra = { extra: kind.journal(view) };
    } catch (err) {
      log(`job ${view.id}: the kind's journal() threw: ${String(err)}`);
    }
  }
  return {
    kind: view.kind,
    family: view.family,
    outcome,
    attempts: view.attempts,
    priority: view.priority,
    warm: view.warm,
    queuedMs,
    runMicros: Math.round(stats.runMicros),
    isolated: stats.isolated,
    helpers: stats.helpers,
    ...(view.error !== undefined && outcome !== "succeeded" ? { code: view.error.code } : {}),
    ...items,
    ...extra,
  };
}
