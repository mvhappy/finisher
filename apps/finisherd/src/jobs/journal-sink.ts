// The job engine's events into the diagnostics journal (D34, spec §6.8 and §21.1; 5C-1 plan
// Decision 42, Task 12): `job.done` and `worker.exit` as source `finisherd`, and forwarded
// `diag` payloads of the journal's own kinds. The journal decides consent (off: nothing is
// recorded). An event the journal refuses (`EVENT_INVALID`, `EVENT_TOO_LARGE`, an unknown
// kind) is logged, never thrown: a `job.done` that would throw is sent again at every start,
// and a refusal does not change on a retry. `worker.exit`'s `plugin` (the keyed hash of a
// plugin id) is left out: the journal's key does not reach TypeScript, and no plugin process
// runs before sub-project 8, which adds the hash where the key lives (a TASKS follow-up).
import type { JobDone, WorkerExit } from "@finisher/core-types";
import type { Journal } from "@finisher/fin-node";
import { kindNames } from "@finisher/fin-node";
import type { ForwardedEvent, JobDoneEvent, JobEventSink, WorkerExitEvent } from "./events.ts";

type Log = (level: "debug" | "warning", message: string) => void;
type JournalLike = Pick<Journal, "enabled" | "record">;

const SOURCE = "finisherd";
let known: ReadonlySet<string> | undefined;

/** `job.done` `kv` 1 (a kind's `journal(job)` extra fields are `kv` 2, sub-project 6). */
export function jobDonePayload(e: JobDoneEvent): JobDone {
  const { extra: _extra, ...payload } = e;
  return payload;
}

/** `worker.exit` `kv` 1, without the plugin id (see above). */
export function workerExitPayload(e: WorkerExitEvent): WorkerExit {
  const { pluginId: _pluginId, ...payload } = e;
  return payload;
}

/** A sink over `journal`; `log` gets what the journal refused. */
export function journalSink(journal: JournalLike, log: Log): JobEventSink {
  const record = (kind: string, payload: unknown, what: string): void => {
    if (!journal.enabled) return;
    try {
      journal.record(kind, 1, null, JSON.stringify(payload), SOURCE);
    } catch (e) {
      log("warning", `job engine: the journal refused ${what}: ${(e as Error).message}`);
    }
  };
  return {
    jobDone: (e, job) => record("job.done", jobDonePayload(e), `job ${job}'s job.done`),
    workerExit: (e) => record("worker.exit", workerExitPayload(e), `a worker.exit (${e.family} ${e.role})`),
    forward: (origin: ForwardedEvent["origin"], kind, payload) => {
      known ??= new Set(kindNames());
      if (!known.has(kind)) {
        log("debug", `job engine: a ${origin.family} worker's diag ${kind} is not a journal kind; dropped`);
        return;
      }
      record(kind, payload, `a ${origin.family} worker's ${kind}`);
    },
  };
}
