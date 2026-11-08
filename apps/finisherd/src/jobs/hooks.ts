// A kind's hooks (5C-1 plan Decision 40; 9b review B1, M3): `prepare` before any process and
// `onSucceeded` after the whole group is gone. Each runs as a fiber of the engine's context,
// bounded by `hookTimeoutS` (60 s) and interrupted by a stop request (`prepare`) or a
// simulated crash. A hook reaches the job's files only through `ctx.open` (`openMapped`: no
// link, FIFO or hard link is ever opened), and before `onSucceeded` the job directory is
// checked for anything a worker planted (`checkNoLinks`, then `checkTree`).
import type { FinError, HelperSpec, JobView, MappedFile } from "@finisher/contracts";
import { Cause, Effect, Exit, Fiber } from "effect";
import { finError, toFinError } from "../errors.ts";
import { checkNoLinks, type JobDirs, jobDirs } from "./files.ts";
import { checkTree, openMapped } from "./fsafe.ts";
import { helperNames } from "./helpers.ts";
import { type KindSpec, resolveOrFn } from "./kinds.ts";
import type { RunControl } from "./run-control.ts";
import type { RunnerEnv } from "./runner.ts";
import type { JobRow } from "./store.ts";

export const DEFAULT_HOOK_TIMEOUT_S = 60;

export type HookOutcome<A> = { ok: true; value: A } | { ok: false; error: FinError };

/** Awaits a fiber's exit. */
export function awaitFiber<A, E>(fiber: Fiber.Fiber<A, E>): Promise<Exit.Exit<A, E>> {
  return new Promise((resolve) => fiber.addObserver(resolve));
}

/**
 * Runs one of a kind's hooks: its value, or its failure as a `FinError` (a defect is
 * `INTERNAL`; past `timeoutS`, `JOB_TIMEOUT {hook, timeoutS}`). A stop request of `control`
 * interrupts it (`JOB_CANCELLED`).
 */
export async function runHook<A>(
  env: RunnerEnv,
  hook: "prepare" | "onSucceeded",
  effect: Effect.Effect<A, FinError>,
  timeoutS: number,
  control?: RunControl,
): Promise<HookOutcome<A>> {
  const bounded = effect.pipe(
    Effect.timeoutOrElse({
      duration: Math.max(0, timeoutS * 1000),
      orElse: () => Effect.fail(finError("JOB_TIMEOUT", `the kind's ${hook} hook ran past ${timeoutS} s`, { hook, timeoutS })),
    }),
  );
  const fiber = env.run(Effect.suspend(() => bounded)) as Fiber.Fiber<unknown, unknown>;
  env.hooks.add(fiber);
  const unbind = control?.bind(() => void env.run(Fiber.interrupt(fiber)));
  const exit = (await awaitFiber(fiber)) as Exit.Exit<A, FinError>;
  unbind?.();
  env.hooks.delete(fiber);
  if (Exit.isSuccess(exit)) return { ok: true, value: exit.value };
  if (Cause.hasInterruptsOnly(exit.cause)) return { ok: false, error: finError("JOB_CANCELLED", `the kind's ${hook} hook was stopped`, { hook }) };
  return { ok: false, error: toFinError(Cause.squash(exit.cause)) };
}

/** What a kind's hooks get: the job (params typed), `open` for its mapped files, the attempt. */
export function hookCtx(view: JobView, dirs: JobDirs, files: ReadonlyArray<MappedFile>, attempt: number) {
  return {
    job: view as JobView & { params: never },
    open: (name: string, how?: "r" | "w" | "rw") => openMapped(dirs.job, files, name, how),
    attempt,
  };
}

/** The kind's mapped files and helpers for the job's own params (a failure is the hook's). */
function layout(kind: KindSpec, row: JobRow): { files: ReadonlyArray<MappedFile>; helpers: ReadonlyArray<HelperSpec> } {
  return { files: resolveOrFn(kind.files ?? [], row.params as never), helpers: resolveOrFn(kind.helpers ?? [], row.params as never) };
}

/**
 * The success path's hook phase (Decision 40). The caller has persisted the result with
 * `phase: hooks` and every process that could write the directory has exited. The directory is
 * checked first (a link, FIFO or hard link a worker planted fails the job, `JOB_FAILED`, and
 * the hook never runs); then `onSucceeded`. Answers the final result (the hook's replacement,
 * if any) or the failure. A restart's re-run (`rerun`) goes the same way.
 */
export async function runOnSucceeded(
  env: RunnerEnv,
  row: JobRow,
  kind: KindSpec,
  result: unknown,
  rerun: boolean,
): Promise<{ ok: true; result: unknown } | { ok: false; error: FinError }> {
  const hook = kind.onSucceeded;
  if (hook === undefined) return { ok: true, result };
  const dirs = jobDirs(env.root, row.id);
  let files: ReadonlyArray<MappedFile>;
  try {
    const l = layout(kind, row);
    files = l.files;
    await checkNoLinks(dirs.job, ["tmp/", ...files.map((f) => f.name), ...helperNames(l.helpers)]);
    await checkTree(dirs.job, files);
  } catch (e) {
    return { ok: false, error: toFinError(e) };
  }
  const ctx = { ...hookCtx(row, dirs, files, row.attempts), result: result as never, items: env.store.items(row.id), rerun };
  const out = await runHook(env, "onSucceeded", Effect.suspend(() => hook(ctx)), kind.hookTimeoutS ?? DEFAULT_HOOK_TIMEOUT_S);
  if (!out.ok) return out;
  return { ok: true, result: out.value === undefined ? result : out.value };
}
