// `job.done`'s `media` object for the `media` kinds (6A plan Task 14, 6B-1 plan Task 19, 6C plan
// Task 22): the worker's account of the job, present only when diagnostics were on at the worker.
// It is passed on as it came; the journal decodes it against the registered `job.done` kind, which
// has `meta` fields only, so nothing else can reach an entry (a malformed one costs the entry only
// its `media`, `journal-sink.ts`).
//
// Where it sits in the result: the batch and cold kinds (`media.proxy`, `media.thumbs`,
// `media.probeDeep`, `media.pcm`, `media.waveform`) answer `{…, media}`; the kinds whose result
// carries a typed `stats` (`media.look`, `media.listen`) answer `{…, stats: {media}}`.
import type { JobView } from "@finisher/contracts";
import type { ItemOutcome } from "../kinds.ts";
import { defaultCombine } from "../retry.ts";

/** `{media}` when `media` is an object, else nothing. */
export function mediaObject(media: unknown): Readonly<Record<string, unknown>> {
  return typeof media === "object" && media !== null && !Array.isArray(media) ? { media } : {};
}

/** Every media kind's `journal`: the result's `media`, else its `stats.media`. */
export function mediaJournal(job: JobView): Readonly<Record<string, unknown>> {
  const result = job.result as { media?: unknown; stats?: { media?: unknown } } | null | undefined;
  return mediaObject(result?.media ?? result?.stats?.media);
}

/**
 * A batch kind's result (`media.proxy`, `media.thumbs`, `media.pcm`, `media.waveform`): the
 * engine's default `{items, itemErrors}` and one `media` for the runs that had one (a batch's own
 * result is otherwise not kept, so `mediaJournal` would find nothing). An isolating retry splits
 * the batch into runs: their `frames` add up; the other fields (codec, backend, throughput, …)
 * are the last run's. Only this attempt's runs count: items acknowledged before a crash carry no
 * media of theirs.
 */
export function mediaCombine(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown {
  const runs = results.map((r) => mediaObject((r as { media?: unknown } | null | undefined)?.media).media as Record<string, unknown> | undefined).filter((m) => m !== undefined);
  const base = defaultCombine(items) as object;
  const last = runs.at(-1);
  if (last === undefined) return base;
  const counts = runs.map((m) => m.frames);
  const frames = counts.every((n) => typeof n === "number" && Number.isInteger(n)) ? (counts as number[]).reduce((a, b) => a + b, 0) : last.frames;
  return { ...base, media: { ...last, frames } };
}
