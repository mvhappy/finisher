// `job.done`'s `media` object for the media kinds (6A Task 14; 6C Task 22): the worker's account
// of the job, `stats.media` of its result (present only when diagnostics were on at the worker).
// It is passed on as it came; the journal decodes it against the registered `job.done` kind, which
// has `meta` fields only, so nothing else can reach an entry.
import type { JobView } from "@finisher/contracts";
import { defaultCombine } from "../retry.ts";
import type { ItemOutcome } from "../kinds.ts";

export function mediaJournal(job: JobView): Readonly<Record<string, unknown>> {
  const media = (job.result as { stats?: { media?: unknown } } | null | undefined)?.stats?.media;
  return typeof media === "object" && media !== null && !Array.isArray(media) ? { media } : {};
}

/**
 * A batch kind's `combine` (Decision 32's default shape) that keeps the worker's `stats` (its
 * `media` object): the last run's, since a retried or isolated job answers once per run.
 */
export function combineWithStats(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown {
  const combined = defaultCombine(items) as Record<string, unknown>;
  for (const r of [...results].reverse()) {
    const stats = (r as { stats?: unknown } | null | undefined)?.stats;
    if (typeof stats === "object" && stats !== null) return { ...combined, stats };
  }
  return combined;
}
