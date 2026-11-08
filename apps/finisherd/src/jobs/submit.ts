// Submit and wait (spec §6.1, 5C-1 plan Decision 27): a submit is validated (kind, params,
// `maxAttempts`, files, pools and stuck pools), answered from an equal `idemKey` or an active
// `dedupeKey`, refused past the queue limit, then inserted, indexed and dispatched in its own
// turn. `wait` resolves once all (or `any`) of the jobs finished, or fails `WAIT_TIMEOUT`.
import type { FinError, JobOwner, JobView } from "@finisher/contracts";
import { validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import { type Engine, finished, MAX_ATTEMPTS_LIMIT, type SubmitSpec } from "./engine.ts";
import { resolveOrFn, validateMappedFiles } from "./kinds.ts";

export function submitJob(e: Engine, owner: JobOwner, spec: SubmitSpec): JobView {
  const { store } = e.d;
  if (e.closing) throw finError("WORKER_UNAVAILABLE", "the job engine is closing", { reason: "closing" });
  const kind = e.kinds.kind(spec.kind);
  if (kind === undefined) throw finError("JOB_KIND_UNKNOWN", `no job kind ${spec.kind}`, { kind: spec.kind });
  const checked = validateJobParams(kind.kind, kind.version, spec.params);
  if (!checked.ok) throw finError("JOB_PARAMS_INVALID", `the params of ${kind.kind} are invalid: ${checked.errors.join("; ")}`, { errors: checked.errors });
  const params = checked.params;
  const max = spec.maxAttempts;
  if (max !== undefined && !(Number.isInteger(max) && max >= 1 && max <= MAX_ATTEMPTS_LIMIT)) {
    const errors = [`maxAttempts: an integer from 1 to ${MAX_ATTEMPTS_LIMIT}, not ${JSON.stringify(max)}`];
    throw finError("JOB_PARAMS_INVALID", `the job's ${errors[0] as string}`, { errors });
  }
  if (kind.files !== undefined && typeof kind.files === "function") {
    const errors = validateMappedFiles(kind.files(params as never));
    if (errors.length > 0) throw finError("JOB_PARAMS_INVALID", `the job's files are invalid: ${errors.join("; ")}`, { errors });
  }
  for (const pool of resolveOrFn(kind.resources, params as never)) {
    if (e.d.pools.size(pool) === undefined) throw finError("WORKER_UNAVAILABLE", `no resource pool ${pool}`, { pool });
    if (e.d.pools.stuck(pool)) {
      throw finError("RESOURCE_STUCK", `the resource pool ${pool} is stuck`, { pool }, "processes on it did not exit when killed: wait for them, or check the disk");
    }
  }
  if (spec.idemKey !== undefined) {
    const same = store.byIdem(owner, spec.idemKey);
    if (same !== undefined) return e.view(same.id) ?? same;
  }
  const dedupeKey = kind.dedupe?.(params as never);
  if (dedupeKey !== undefined) {
    const active = store.activeByDedupe(kind.kind, dedupeKey) ?? (kind.rememberPoison === true ? store.poisonedByDedupe(kind.kind, dedupeKey) : undefined);
    if (active !== undefined) return e.view(active.id) ?? active;
  }
  if (e.unfinished >= e.d.queueLimit) {
    throw finError("JOB_QUEUE_FULL", "the job queue is full", { limit: e.d.queueLimit }, "wait for jobs to finish, or cancel some");
  }
  const priority = spec.priority ?? kind.defaultPriority;
  const view = store.insert({
    kind: kind.kind,
    kindVersion: kind.version,
    family: kind.family,
    params,
    owner,
    priority,
    maxAttempts: max ?? kind.maxAttempts ?? (priority === "interactive" ? 2 : 3),
    ...(spec.via === undefined ? {} : { via: spec.via }),
    ...(spec.rights === undefined ? {} : { rights: spec.rights }),
    ...(spec.project === undefined ? {} : { project: spec.project }),
    ...(spec.library === undefined ? {} : { library: spec.library }),
    ...(spec.parent === undefined ? {} : { parent: spec.parent }),
    ...(spec.idemKey === undefined ? {} : { idemKey: spec.idemKey }),
    ...(dedupeKey === undefined ? {} : { dedupeKey }),
  });
  e.unfinished++;
  e.publish({ type: "state", job: view });
  e.enqueue(view, kind);
  // A warm job with a ready worker of its key goes to it now, without `schedule()` (Decision 35).
  if (!e.warmPools.handOff(view.id)) e.pump();
  return e.view(view.id) ?? view;
}

export function waitJobs(e: Engine, ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError> {
  return Effect.suspend(() => {
    for (const id of ids) if (e.d.store.get(id) === undefined) return Effect.fail(finError("JOB_NOT_FOUND", `no job ${id}`, { job: id }));
    const check = (): JobView[] | undefined => {
      const views = ids.map((id) => e.view(id) as JobView);
      const n = views.filter(finished).length;
      return (opts.any === true ? n > 0 : n === views.length) ? views : undefined;
    };
    const now = check();
    if (now !== undefined) return Effect.succeed(now);
    return Effect.callback<ReadonlyArray<JobView>, FinError>((resume) => {
      let cancel = (): void => {};
      const cleanup = (): void => {
        cancel();
        e.waiters.delete(onDone);
      };
      const onDone = (): void => {
        const views = check();
        if (views === undefined) return;
        cleanup();
        resume(Effect.succeed(views));
      };
      cancel = e.d.timers.after(opts.timeoutMs, () => {
        cleanup();
        resume(Effect.fail(finError("WAIT_TIMEOUT", `the jobs did not finish within ${opts.timeoutMs} ms`, { jobs: ids, timeoutMs: opts.timeoutMs })));
      });
      e.waiters.add(onDone);
      return Effect.sync(cleanup);
    });
  });
}
