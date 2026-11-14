// `job.done`'s `media` object for the `media` kinds (6A plan Task 14, 6B-1 plan Task 19): the
// worker's account of the job, present only when diagnostics were on at the worker. It is passed
// on as it came; the journal decodes it against the registered `job.done` kind, which has `meta`
// fields only, so nothing else can reach an entry (a malformed one costs the entry only its
// `media`, `journal-sink.ts`).
import type { JobView } from "@finisher/contracts";
import type { ItemOutcome } from "../kinds.ts";
import { defaultCombine } from "../retry.ts";

/** `{media}` when `media` is an object, else nothing. */
export function mediaObject(media: unknown): Readonly<Record<string, unknown>> {
  return typeof media === "object" && media !== null && !Array.isArray(media) ? { media } : {};
}

/** The cold kinds' (`media.proxy`, `media.thumbs`, `media.probeDeep`): the result's `media`. */
export function mediaJournal(job: JobView): Readonly<Record<string, unknown>> {
  return mediaObject((job.result as { media?: unknown } | null | undefined)?.media);
}

/**
 * A batch kind's result (`media.proxy`, `media.thumbs`): the engine's default `{items,
 * itemErrors}` and the `media` of the last run that had one (a batch's own result is otherwise
 * not kept, so `mediaJournal` would find nothing).
 */
export function mediaCombine(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown {
  const last = [...results].reverse().map((r) => mediaObject((r as { media?: unknown } | null | undefined)?.media)).find((m) => "media" in m);
  return { ...(defaultCombine(items) as object), ...last };
}
