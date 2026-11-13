// The job engine's events into the diagnostics journal (D34, spec §6.8 and §21.1; 5C-1 plan
// Decision 42, Task 12): `job.done` and `worker.exit` as source `finisherd`, and forwarded
// `diag` payloads of the journal's own kinds. The journal decides consent (off: nothing is
// recorded). An event the journal refuses (`EVENT_INVALID`, `EVENT_TOO_LARGE`, an unknown
// kind) is logged, never thrown: a `job.done` that would throw is sent again at every start,
// and a refusal does not change on a retry. `worker.exit` is `kv` 2 (sp8b Task 15, Decision
// 22): a plugin process's exit goes through `journal.recordPlugin` with the raw plugin id,
// which fin-node names by its `PluginRef` (a bundled name or the journal's keyed hash: the key
// never reaches TypeScript) and from which it also records the exit's `plugin.exit`.
import type { JobDone, JobMedia, WorkerExit } from "@finisher/core-types";
import type { Journal } from "@finisher/fin-node";
import { kindNames } from "@finisher/fin-node";
import { type ForwardedEvent, type JobDoneEvent, type JobEventSink, lifetimeSeconds, type WorkerExitEvent } from "./events.ts";

type Log = (level: "debug" | "warning", message: string) => void;
type JournalLike = Pick<Journal, "enabled" | "record" | "recordPlugin">;

const SOURCE = "finisherd";
const JOB_DONE_KV = 2;
const WORKER_EXIT_KV = 2;
let known: ReadonlySet<string> | undefined;

/**
 * `job.done` `kv` 2: the engine's fields and, from a kind's `journal(job)` extras, the `media`
 * object of sub-project 6 (no other extra key reaches the journal). The journal decodes it
 * against the registered kind.
 */
export function jobDonePayload(e: JobDoneEvent): JobDone {
  const { extra, ...payload } = e;
  const media = extra?.["media"];
  return media === undefined ? payload : { ...payload, media: media as JobMedia };
}

/**
 * `worker.exit` `kv` 2 without `plugin` (fin-node adds it from the raw id, see above).
 * `lifetimeS` is whole seconds: a fraction would fail the journal's `u64` and lose the entry.
 */
export function workerExitPayload(e: WorkerExitEvent): Omit<WorkerExit, "plugin"> {
  const { pluginId: _pluginId, ...payload } = e;
  return Number.isInteger(payload.lifetimeS) ? payload : { ...payload, lifetimeS: lifetimeSeconds(payload.lifetimeS * 1000) };
}

/** A sink over `journal`; `log` gets what the journal refused. */
export function journalSink(journal: JournalLike, log: Log): JobEventSink {
  const record = (kind: string, kv: number, payload: unknown, what: string): boolean => {
    if (!journal.enabled) return true;
    try {
      journal.record(kind, kv, null, JSON.stringify(payload), SOURCE);
      return true;
    } catch (e) {
      log("warning", `job engine: the journal refused ${what}: ${(e as Error).message}`);
      return false;
    }
  };
  return {
    jobDone: (e, job) => {
      const payload = jobDonePayload(e);
      const what = `job ${job}'s job.done`;
      // A worker's malformed `media` object must not cost the job its `job.done`.
      if (!record("job.done", JOB_DONE_KV, payload, what) && payload.media !== undefined) {
        const { media: _media, ...plain } = payload;
        record("job.done", JOB_DONE_KV, plain, `${what} without its media object`);
      }
    },
    workerExit: (e) => {
      const what = `a worker.exit (${e.family} ${e.role})`;
      if (e.pluginId === undefined) {
        record("worker.exit", WORKER_EXIT_KV, workerExitPayload(e), what);
        return;
      }
      if (!journal.enabled) return;
      try {
        journal.recordPlugin("worker.exit", WORKER_EXIT_KV, e.pluginId, JSON.stringify(workerExitPayload(e)));
      } catch (err) {
        log("warning", `job engine: the journal refused ${what}: ${(err as Error).message}`);
      }
    },
    forward: (origin: ForwardedEvent["origin"], kind, payload) => {
      known ??= new Set(kindNames());
      if (!known.has(kind)) {
        log("debug", `job engine: a ${origin.family} worker's diag ${kind} is not a journal kind; dropped`);
        return;
      }
      record(kind, 1, payload, `a ${origin.family} worker's ${kind}`);
    },
  };
}
