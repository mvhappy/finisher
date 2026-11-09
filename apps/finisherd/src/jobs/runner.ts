// One attempt of one job (spec §6.2–6.4, 5C-1 plan Tasks 9a, 9b, 11): the job directory and
// its files → `prepare` (until it succeeds once per directory) → the worker's part. For a cold
// job (here): the helpers (hello, attach) → the worker in the job's group (hello, kinds check,
// start) → its messages → result, error or exit → the helpers shut down → the group gone; a
// warm job's part is in `warm-run.ts`, what both share in `attempt.ts`. Then, for a success, the result and
// `phase: hooks` are persisted and `onSucceeded` runs (Decision 40). The attempt's outcome
// carries what Task 10's isolation needs: the items acknowledged in this run and the items
// begun without an `item` (a set: a family may acknowledge a group of items after one reply,
// e.g. 5D's probe groups of up to 32). A `RunControl` stops the attempt (cancel, pause, close).
import type { Hello, HelperGrant } from "@finisher/core-types";
import type { FinError, HelperSpec, JobProgress, JobView, MappedFile } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { Effect, type Fiber } from "effect";
import { finError, toFinError } from "../errors.ts";
import { childEnv, passEnv } from "../env.ts";
import { AttemptBase, type AttemptSpec, type Facts, helloMismatch, type KillReason, type Outcome, stopped } from "./attempt.ts";
import type { JobEventSink } from "./events.ts";
import type { RunFork } from "./calls.ts";
import { DEFAULT_HOOK_TIMEOUT_S, hookCtx, runHook } from "./hooks.ts";
import type { RunControl } from "./run-control.ts";
import { checkNoLinks, checkPrepared, createFiles, createJobDir, isPrepared, type JobDirs, jobDirs, mappedPath, markPrepared } from "./files.ts";
import { helperNames } from "./helpers.ts";
import { type EngineHost, type JobHostExitInfo, type JobSpawnRequest, type ProcEvent, exitVerdict } from "./host.ts";
import { type FamilySpec, type ItemOutcome, type KindRegistry, type KindSpec, type MemoryRule, resolveOrFn } from "./kinds.ts";
import type { JobRow } from "./store.ts";
import type { WriterEnv } from "./writers.ts";

export { DEFAULT_STALL_S, MAX_ITEM_CHARS } from "./attempt.ts";

export const ALIVE_MS = 5000;
/** After a worker's answer, how long it may take to exit before its group is killed. */
export const EXIT_GRACE_MS = 2000;

type Log = (level: "debug" | "warning" | "error", message: string) => void;

/** What a runner needs from its engine. */
export interface RunnerEnv extends WriterEnv {
  readonly host: EngineHost;
  readonly registry: KindRegistry;
  readonly sink: JobEventSink;
  /** `<server-data>/jobs`. */
  readonly root: string;
  readonly totalMem: number;
  readonly run: RunFork;
  readonly diagnostics: () => boolean;
  /** fin-node is a test-hooks build: a non-`test` family that leaves foreign processes fails (Decision 22). */
  readonly testHooks: boolean;
  /** After a warm worker's `ready`, the files it holds under the job directory are checked (Decision 36). */
  readonly heldFilesCheck: boolean;
  readonly aliveMs: number;
  readonly log: Log;
  /** Hook fibers in flight (a simulated crash interrupts them). */
  readonly hooks: Set<Fiber.Fiber<unknown, unknown>>;
  progress(job: string, p: JobProgress): void;
  item(job: string, item: string, outcome: ItemOutcome): void;
  diagDropped(): void;
  /** A process of the job was abandoned (Decision 18). */
  abandoned(proc: number, job: string): void;
}

/** Why an attempt failed, for the retry policy (Task 10 classifies it, Decision 31). */
export type FailureCause =
  | "files"
  | "tampered"
  | "prepare"
  | "spawn"
  | "mismatch"
  | "worker"
  | "exit"
  | "foreign"
  | "helper"
  | "protocol"
  | "stopped"
  /** A warm worker held a file of the job directory past its `ready` (Decision 36, test-hooks builds). */
  | "heldFiles";

export type AttemptOutcome = AttemptFacts & Outcome;

export interface AttemptFacts {
  /** Items acknowledged (`item`) in this run, in order. */
  readonly done: ReadonlyArray<string>;
  /** Items begun and not acknowledged when the run ended, in `begin` order (a set). */
  readonly inFlight: ReadonlyArray<string>;
  readonly runMicros: number;
  /** Helper processes started. */
  readonly helpers: number;
  /** A stop request reached the attempt before the worker answered (9b review M1). */
  readonly stopSent: boolean;
  /** The attempt ran on a warm worker that had served jobs before (Decision 31's `oomWarm`). */
  readonly warmReused?: boolean;
}

/** The memory rule of a job's group: the kind's, else the family's for its class (Decision 20). */
export function memoryRule(kind: KindSpec, family: FamilySpec, view: JobView): MemoryRule | undefined {
  if (kind.memory !== undefined) return resolveOrFn(kind.memory, view.params as never);
  return view.priority === "interactive" ? family.memory?.interactive : family.memory?.batch;
}

/** An attempt's job directory, files and `prepare` done: what the worker's part starts from. */
export interface Prepared {
  readonly dirs: JobDirs;
  readonly files: ReadonlyArray<MappedFile>;
  readonly helpers: ReadonlyArray<HelperSpec>;
  readonly extra: unknown;
  readonly facts: Facts;
}

/**
 * One attempt of the running job `row` (already `running`, its `attempts` counting this one):
 * the job directory, its files and `prepare`, then `body`, the worker's part. `params` are
 * what the worker runs (a batch's remaining items).
 */
export async function attemptWith(
  env: RunnerEnv,
  row: JobRow,
  kind: KindSpec,
  params: unknown,
  control: RunControl,
  body: (p: Prepared) => Promise<Outcome>,
): Promise<AttemptOutcome> {
  const t0 = performance.now();
  const facts: Facts = { done: [], inFlight: new Set<string>(), helpers: 0, stopSent: false };
  const finish = (o: Outcome): AttemptOutcome => ({
    ...o,
    done: facts.done,
    inFlight: [...facts.inFlight],
    helpers: facts.helpers,
    stopSent: facts.stopSent,
    runMicros: (performance.now() - t0) * 1000,
  });
  const dirs = jobDirs(env.root, row.id);
  let files: ReadonlyArray<MappedFile>;
  let helpers: ReadonlyArray<HelperSpec>;
  let extra = row.extra;
  try {
    files = resolveOrFn(kind.files ?? [], params as never);
    helpers = resolveOrFn(kind.helpers ?? [], params as never);
    // A directory a worker used may hold links it planted: refused before anything follows them (9a review B1).
    if (!(await createJobDir(dirs))) await checkNoLinks(dirs.job, ["tmp/", ...files.map((f) => f.name), ...helperNames(helpers)]);
  } catch (e) {
    const err = toFinError(e);
    return finish({ ok: false, error: err, cause: err.code === "JOB_FAILED" ? "tampered" : "files" });
  }
  try {
    await createFiles(dirs.job, files);
    // `prepare` runs until it succeeded once for this directory (9a review M3), or before every
    // attempt when the kind asks; bounded and stoppable (9b review M3).
    if (kind.prepare !== undefined && (kind.prepareEachAttempt === true || !(await isPrepared(dirs)))) {
      const prepare = kind.prepare;
      const ctx = hookCtx(row, dirs, files, row.attempts);
      const out = await runHook(env, "prepare", Effect.suspend(() => prepare(ctx)), kind.hookTimeoutS ?? DEFAULT_HOOK_TIMEOUT_S, control);
      if (control.requested !== undefined) return finish({ ok: false, error: stopped(), cause: "stopped" });
      if (!out.ok) return finish({ ok: false, error: out.error, cause: "prepare" });
      const value = out.value as { extra?: unknown } | undefined;
      if (value?.extra !== undefined && !env.dead()) {
        extra = value.extra;
        env.store.transition(row.id, "running", { extra });
      }
      await markPrepared(dirs);
    }
    await checkPrepared(dirs.job, files);
  } catch (e) {
    const err = toFinError(e);
    return finish({ ok: false, error: err, cause: err.code === "JOB_FAILED" ? "tampered" : "files" });
  }
  if (control.requested !== undefined) return finish({ ok: false, error: stopped(), cause: "stopped" });
  return body({ dirs, files, helpers, extra, facts }).then(finish);
}

/** One attempt of a cold job: its worker spawned in the job's group, ended by its exit. */
export function runAttempt(env: RunnerEnv, row: JobRow, kind: KindSpec, family: FamilySpec, params: unknown, control: RunControl): Promise<AttemptOutcome> {
  return attemptWith(env, row, kind, params, control, (p) => new ColdAttempt({ env, row, kind, family, params, control, ...p }).run(p.helpers));
}

class ColdAttempt extends AttemptBase {
  #group = -1;
  #foreign = 0;
  #cancelExit: (() => void) | undefined;

  constructor(a: AttemptSpec) {
    super(a, () => this.#group);
  }

  protected kill(reason: KillReason): void {
    void this.a.env.host.killGroup(this.#group, reason).catch(() => {});
  }

  async run(helpers: ReadonlyArray<HelperSpec>): Promise<Outcome> {
    const { env, row, kind, family, control } = this.a;
    const rule = memoryRule(kind, family, row);
    const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, env.totalMem);
    const qos = row.priority === "interactive" ? "normal" : kind.qos;
    this.#group = env.host.openGroup({ label: `${kind.kind} ${row.id}`, qos, ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    const done = new Promise<Outcome>((resolve) => {
      this.settle = resolve;
    });
    this.unbind = control.bind((reason, graceMs) => this.stop(reason, graceMs));
    let grants: HelperGrant[];
    try {
      grants = await this.helpers.start(helpers);
    } catch (e) {
      this.a.facts.helpers = this.helpers.started;
      await this.#endGroup();
      if (control.requested !== undefined) return { ok: false, error: stopped(), cause: "stopped" };
      // The helper's own failure counts, not a later error it caused (9b review L3).
      return this.fault === undefined ? { ok: false, error: toFinError(e), cause: "helper" } : { ok: false, ...this.fault };
    }
    this.a.facts.helpers = this.helpers.started;
    if (control.requested !== undefined || this.fault !== undefined) {
      await this.#endGroup();
      return this.fault === undefined ? { ok: false, error: stopped(), cause: "stopped" } : { ok: false, ...this.fault };
    }
    try {
      await env.host.spawn(this.#group, this.#request(), (e, proc) => this.#onEvent(e, proc, memoryBytes, grants), row.id);
    } catch (e) {
      await this.#endGroup();
      return { ok: false, error: toFinError(e), cause: "spawn" };
    }
    return done;
  }

  async #endGroup(): Promise<void> {
    this.unbind();
    try {
      await this.a.env.host.killGroup(this.#group, "jobEnded");
    } catch (e) {
      this.a.env.log("warning", `job ${this.a.row.id}: ending its group failed: ${toFinError(e).message}`);
    }
    await this.helpers.closeCalls();
    this.a.env.host.closeGroup(this.#group);
  }

  #request(): JobSpawnRequest {
    const { env, row, family, kind, dirs, files } = this.a;
    const program = env.registry.program(family.family, row, "cold");
    // A plugin family starts its process its own way (sp8b Task 4): its environment, `cwd`,
    // grants and limits, never the job directory's `tmp/`.
    const own = "path" in program ? program.process : undefined;
    return {
      role: "worker",
      profile: family.profile,
      family: family.family,
      program: "path" in program ? { path: program.path } : { bundled: program.binary },
      args: [...program.args],
      env: own?.env ?? { ...childEnv(), ...passEnv(family.env), TMPDIR: dirs.tmp, TEMP: dirs.tmp, TMP: dirs.tmp },
      cwd: own?.cwd ?? dirs.job,
      // `tmp/` read-write and each mapped file at its own mode (9a review H1); not the job
      // directory itself, which holds the helpers' own directories (9b review L4).
      grants: [
        ...(own?.grants ?? [{ path: dirs.tmp, mode: "readWrite" as const }]),
        ...files.map((f) => ({ path: mappedPath(dirs.job, f.name), mode: f.mode })),
        // What the worker reads outside its directory (sources, caches; plan 7C Task 7).
        ...(kind.reads?.(this.a.extra, this.a.params as never) ?? []).map((path) => ({ path, mode: "read" as const })),
      ],
      ...(own?.limits === undefined ? {} : { limits: own.limits }),
      qos: row.priority === "interactive" ? "normal" : kind.qos,
      label: `${kind.kind} ${row.id}`,
    };
  }

  #onEvent(e: ProcEvent, proc: number, memoryBytes: number | undefined, grants: HelperGrant[]): void {
    this.proc ??= proc;
    switch (e.type) {
      case "hello":
        // F2: asked for a sandbox, applied nothing (the host accepts it only where Landlock is off).
        if (e.hello.selfSandbox === "none") this.a.env.log("warning", `[job ${this.a.row.id} ${this.a.family.family}] the worker runs unconfined: the sandbox mechanism is disabled on this host`);
        return this.#onHello(e.hello, memoryBytes, grants);
      case "message":
        return this.onMessage(e.msg);
      case "stderr":
        return this.a.env.log("debug", `[job ${this.a.row.id} ${this.a.family.family}] ${e.line}`);
      case "exit":
        return void this.#onExit(e.info);
    }
  }

  #onHello(hello: Hello, memoryBytes: number | undefined, grants: HelperGrant[]): void {
    const { kind, family, control } = this.a;
    if (this.started || this.fault !== undefined || this.proc === undefined) return;
    if (control.requested !== undefined) {
      this.kill("cancel");
      return;
    }
    const mismatch = helloMismatch(hello, family, kind, false);
    if (mismatch !== undefined) {
      this.failWith(mismatch, "mismatch", "jobEnded");
      return;
    }
    this.sendStart(memoryBytes, grants);
  }

  protected afterAnswer(): void {
    // A cold worker exits once it answered; one that does not is ended with its group.
    this.#cancelExit = this.a.env.timers.after(EXIT_GRACE_MS, () => this.kill("jobEnded"));
  }

  async #onExit(info: JobHostExitInfo): Promise<void> {
    const { env, row, family } = this.a;
    let outcome: Outcome | undefined;
    try {
      this.#cancelExit?.();
      this.cancelKill?.();
      if (info.reason === "abandoned" && this.proc !== undefined) env.abandoned(this.proc, row.id);
      // Open host calls end before anything that reads the directory runs (9a review M1).
      await this.calls.close();
      const verdict = exitVerdict(info, this.answer !== undefined);
      this.workerExit(info, verdict, this.started ? 1 : 0);
      this.#foreign = info.foreign ?? 0;
      if (this.answer !== undefined && "value" in this.answer) await this.helpers.shutdown();
      // Every process of the group, the worker's leftovers and the helpers too, is gone before the outcome counts.
      await this.#endGroup();
      if (this.#foreign > 0) env.log("warning", `job ${row.id} (${family.family}) left ${this.#foreign} foreign process(es)`);
      outcome = this.#outcome(info, verdict.error);
    } catch (e) {
      outcome = { ok: false, error: finError("INTERNAL", `the attempt's end failed: ${toFinError(e).message}`), cause: "exit", exit: info };
    } finally {
      this.flush();
      this.settle(outcome ?? { ok: false, error: finError("INTERNAL", "the attempt ended without an outcome"), cause: "exit", exit: info });
    }
  }

  #outcome(info: JobHostExitInfo, exitError: FinError): Outcome {
    const { env, family } = this.a;
    const a = this.answer;
    if (this.fault === undefined && a !== undefined && "value" in a && env.testHooks && this.#foreign > 0 && family.family !== "test") {
      return { ok: false, error: finError("WORKER_CRASHED", "the worker started processes", { foreign: this.#foreign }), cause: "foreign", exit: info };
    }
    return this.outcomeOf(info, exitError);
  }
}
