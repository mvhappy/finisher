// One attempt of one cold job (spec §6.2–6.4, 5C-1 plan Tasks 9a, 9b): the job directory and
// its files → `prepare` (until it succeeds once per directory) → the helpers (hello, attach) →
// the worker in the job's group (hello, kinds check, start) → its messages → result, error or
// exit → the helpers shut down → the group gone. Then, for a success, the result and
// `phase: hooks` are persisted and `onSucceeded` runs (Decision 40). The attempt's outcome
// carries what Task 10's isolation needs: the items acknowledged in this run and the items
// begun without an `item` (a set: a family may acknowledge a group of items after one reply,
// e.g. 5D's probe groups of up to 32). A `RunControl` stops the attempt (cancel, pause, close).
import type { CancelReason, Hello, HelperGrant, HostMsg, JobError as JobErrorMsg, Retry, StartJob, WorkerMsg } from "@finisher/core-types";
import { FinError, type HelperSpec, type JobProgress, type JobView, type MappedFile } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { Cause, Effect, Exit, type Fiber } from "effect";
import { finError, toFinError } from "../errors.ts";
import { childEnv } from "../env.ts";
import type { JobEventSink, WorkerExitEvent } from "./events.ts";
import { CallRouter, type RunFork } from "./calls.ts";
import { checkNoLinks, checkPrepared, createFiles, createJobDir, isPrepared, type JobDirs, jobDirs, mappedPath, markPrepared } from "./files.ts";
import { Helpers, helperNames } from "./helpers.ts";
import { type EngineHost, type JobHostExitInfo, type JobSpawnRequest, type ProcEvent, exitVerdict } from "./host.ts";
import { type FamilySpec, type ItemOutcome, type KindRegistry, type KindSpec, type MemoryRule, resolveOrFn } from "./kinds.ts";
import type { JobRow } from "./store.ts";
import { CheckpointWriter, ItemWriter, type WriterEnv } from "./writers.ts";

export const ALIVE_MS = 5000;
export const DEFAULT_STALL_S = 600;
export const DEFAULT_HOOK_TIMEOUT_S = 60;
/** After a worker's answer, how long it may take to exit before its group is killed. */
export const EXIT_GRACE_MS = 2000;
/** The longest item name a worker may report (a longer one is a protocol error). */
export const MAX_ITEM_CHARS = 1024;

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
export type FailureCause = "files" | "tampered" | "prepare" | "spawn" | "mismatch" | "worker" | "exit" | "foreign" | "helper" | "protocol" | "stopped";

type Outcome =
  | { readonly ok: true; readonly value: unknown }
  | {
      readonly ok: false;
      readonly error: FinError;
      readonly cause: FailureCause;
      /** A worker `error`'s retry hint. */
      readonly retry?: Retry;
      readonly exit?: JobHostExitInfo;
    };

export type AttemptOutcome = AttemptFacts & Outcome;

export interface AttemptFacts {
  /** Items acknowledged (`item`) in this run, in order. */
  readonly done: ReadonlyArray<string>;
  /** Items begun and not acknowledged when the run ended, in `begin` order (a set). */
  readonly inFlight: ReadonlyArray<string>;
  readonly runMicros: number;
  /** Helper processes started. */
  readonly helpers: number;
}

/** Why a run is stopped: `close` pauses it for the next start (Decision 29). */
export type StopReason = "cancel" | "pause" | "close";
const STOP_RANK: Record<StopReason, number> = { pause: 0, close: 1, cancel: 2 };

/** The engine's handle on a running attempt: the strongest request wins (cancel > close > pause). */
export class RunControl {
  requested: StopReason | undefined;
  #stop: ((reason: StopReason, graceMs: number) => void) | undefined;
  #grace = 0;

  request(reason: StopReason, graceMs: number): void {
    if (this.requested !== undefined && STOP_RANK[this.requested] >= STOP_RANK[reason]) return;
    this.requested = reason;
    this.#grace = graceMs;
    this.#stop?.(reason, graceMs);
  }

  /** The attempt's stop: called at once for a request made before. */
  bind(stop: (reason: StopReason, graceMs: number) => void): void {
    this.#stop = stop;
    if (this.requested !== undefined) stop(this.requested, this.#grace);
  }
}

/** The memory rule of a job's group: the kind's, else the family's for its class (Decision 20). */
export function memoryRule(kind: KindSpec, family: FamilySpec, view: JobView): MemoryRule | undefined {
  if (kind.memory !== undefined) return resolveOrFn(kind.memory, view.params as never);
  return view.priority === "interactive" ? family.memory?.interactive : family.memory?.batch;
}

const jobErr = (e: JobErrorMsg): FinError => new FinError({ code: e.code, message: e.message, details: e.details ?? {} });

/** Awaits a fiber's exit. */
export function awaitFiber<A, E>(fiber: Fiber.Fiber<A, E>): Promise<Exit.Exit<A, E>> {
  return new Promise((resolve) => fiber.addObserver(resolve));
}

/** Runs one of a kind's hooks: its value, or its failure as a `FinError` (a defect is `INTERNAL`). */
async function runHook<A>(env: RunnerEnv, effect: Effect.Effect<A, FinError>, timeoutS?: number): Promise<{ ok: true; value: A } | { ok: false; error: FinError }> {
  const bounded =
    timeoutS === undefined
      ? effect
      : effect.pipe(
          Effect.timeoutOrElse({
            duration: Math.max(0, timeoutS * 1000),
            orElse: () => Effect.fail(finError("JOB_TIMEOUT", `the kind's hook ran past ${timeoutS} s`, { hook: "onSucceeded", timeoutS })),
          }),
        );
  const fiber = env.run(Effect.suspend(() => bounded)) as Fiber.Fiber<unknown, unknown>;
  env.hooks.add(fiber);
  const exit = (await awaitFiber(fiber)) as Exit.Exit<A, FinError>;
  env.hooks.delete(fiber);
  if (Exit.isSuccess(exit)) return { ok: true, value: exit.value };
  return { ok: false, error: toFinError(Cause.squash(exit.cause)) };
}

/** The job's view with its params typed for a kind's hooks. */
function hookCtx(view: JobView, dirs: JobDirs, attempt: number) {
  return { job: view as JobView & { params: never }, dir: dirs.job, path: (name: string) => mappedPath(dirs.job, name), attempt };
}

/**
 * The success path's hook phase (Decision 40). The caller has persisted the result with
 * `phase: hooks` and every process that could write the directory has exited. Answers the
 * final result (the hook's replacement, if any) or the hook's failure.
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
  const ctx = { ...hookCtx(row, dirs, row.attempts), result: result as never, items: env.store.items(row.id), rerun };
  const out = await runHook(env, Effect.suspend(() => hook(ctx)), kind.hookTimeoutS ?? DEFAULT_HOOK_TIMEOUT_S);
  if (!out.ok) return out;
  return { ok: true, result: out.value === undefined ? result : out.value };
}

const stopped = (): FinError => finError("JOB_CANCELLED", "the job was cancelled");

interface Facts {
  done: string[];
  inFlight: Set<string>;
  helpers: number;
}

/**
 * One attempt of the running job `row` (already `running`, its `attempts` counting this one).
 * `params` are what the worker runs (a batch's remaining items).
 */
export async function runAttempt(env: RunnerEnv, row: JobRow, kind: KindSpec, family: FamilySpec, params: unknown, control: RunControl): Promise<AttemptOutcome> {
  const t0 = performance.now();
  const facts: Facts = { done: [], inFlight: new Set<string>(), helpers: 0 };
  const finish = (o: Outcome): AttemptOutcome => ({ ...o, done: facts.done, inFlight: [...facts.inFlight], helpers: facts.helpers, runMicros: (performance.now() - t0) * 1000 });
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
    // `prepare` runs until it succeeded once for this directory (9a review M3).
    if (kind.prepare !== undefined && !(await isPrepared(dirs))) {
      const prepare = kind.prepare;
      const out = await runHook(env, Effect.suspend(() => prepare(hookCtx(row, dirs, row.attempts))));
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
    return finish({ ok: false, error: toFinError(e), cause: "files" });
  }
  if (control.requested !== undefined) return finish({ ok: false, error: stopped(), cause: "stopped" });
  return new Attempt({ env, row, kind, family, params, dirs, files, extra, facts, control }).run(helpers).then(finish);
}

type Answer = { readonly value: unknown } | { readonly error: JobErrorMsg; readonly retry: Retry };

interface AttemptSpec {
  readonly env: RunnerEnv;
  readonly row: JobRow;
  readonly kind: KindSpec;
  readonly family: FamilySpec;
  readonly params: unknown;
  readonly dirs: JobDirs;
  readonly files: ReadonlyArray<MappedFile>;
  readonly extra: unknown;
  readonly facts: Facts;
  readonly control: RunControl;
}

class Attempt {
  readonly #a: AttemptSpec;
  readonly #items: ItemWriter;
  readonly #checkpoints: CheckpointWriter;
  readonly #calls: CallRouter;
  readonly #helpers: Helpers;
  #group = -1;
  #proc: number | undefined;
  #started = false;
  #answer: Answer | undefined;
  /** Why the attempt failed before or beside the worker's own outcome (the first one counts). */
  #fault: { error: FinError; cause: FailureCause } | undefined;
  #foreign = 0;
  #cancelExit: (() => void) | undefined;
  #cancelKill: (() => void) | undefined;
  #diagnostics = false;
  #settle: (o: Outcome) => void = () => {};

  constructor(a: AttemptSpec) {
    this.#a = a;
    const { env, row, family } = a;
    this.#items = new ItemWriter(env, row.id);
    this.#checkpoints = new CheckpointWriter(env, row.id);
    this.#calls = new CallRouter(
      family.onCall,
      () => ({
        job: env.store.get(row.id) ?? row,
        role: "worker",
        owner: row.owner,
        ...(row.via === undefined ? {} : { via: row.via }),
        ...(row.rights === undefined ? {} : { rights: row.rights }),
      }),
      (msg) => (this.#proc === undefined ? "gone" : env.host.send(this.#proc, msg)),
      env.run,
    );
    const qos = row.priority === "interactive" ? "normal" : a.kind.qos;
    this.#helpers = new Helpers(env, row, a.dirs, a.files, { group: () => this.#group, qos, fail: (e) => this.#failWith(e, "helper", "helperFailed") });
  }

  /** Records the attempt's failure (the first one counts) and ends the job's group. */
  #failWith(error: FinError, cause: FailureCause, kill: "helperFailed" | "jobEnded"): void {
    if (this.#fault !== undefined || this.#answer !== undefined) return;
    this.#fault = { error, cause };
    void this.#a.env.host.killGroup(this.#group, kill).catch(() => {});
  }

  async run(helpers: ReadonlyArray<HelperSpec>): Promise<Outcome> {
    const { env, row, kind, family, control } = this.#a;
    const rule = memoryRule(kind, family, row);
    const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, env.totalMem);
    const qos = row.priority === "interactive" ? "normal" : kind.qos;
    this.#group = env.host.openGroup({ label: `${kind.kind} ${row.id}`, qos, ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    const done = new Promise<Outcome>((resolve) => {
      this.#settle = resolve;
    });
    control.bind((reason, graceMs) => this.#stop(reason, graceMs));
    let grants: HelperGrant[];
    try {
      grants = await this.#helpers.start(helpers);
    } catch (e) {
      this.#a.facts.helpers = this.#helpers.started;
      await this.#endGroup();
      return { ok: false, error: toFinError(e), cause: control.requested !== undefined ? "stopped" : "helper" };
    }
    this.#a.facts.helpers = this.#helpers.started;
    if (control.requested !== undefined || this.#fault !== undefined) {
      await this.#endGroup();
      return this.#fault === undefined ? { ok: false, error: stopped(), cause: "stopped" } : { ok: false, ...this.#fault };
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
    try {
      await this.#a.env.host.killGroup(this.#group, "jobEnded");
    } catch (e) {
      this.#a.env.log("warning", `job ${this.#a.row.id}: ending its group failed: ${toFinError(e).message}`);
    }
    await this.#helpers.closeCalls();
    this.#a.env.host.closeGroup(this.#group);
  }

  /** Cancel or pause (Decisions 23, 37): `cancel` to a started worker, its group killed after the grace. */
  #stop(reason: StopReason, graceMs: number): void {
    const { env, row } = this.#a;
    if (this.#answer !== undefined) return; // it answered: the attempt ends on its own
    if (this.#started && this.#proc !== undefined) {
      const wire: CancelReason = reason === "cancel" ? "cancel" : "pause";
      env.host.send(this.#proc, { t: "cancel", job: row.id, reason: wire });
      this.#cancelKill?.();
      this.#cancelKill = env.timers.after(graceMs, () => void env.host.killGroup(this.#group, "cancel").catch(() => {}));
    } else void env.host.killGroup(this.#group, "cancel").catch(() => {});
  }

  #request(): JobSpawnRequest {
    const { env, row, family, kind, dirs, files } = this.#a;
    const program = env.registry.program(family.family, row, "cold");
    return {
      role: "worker",
      profile: family.profile,
      family: family.family,
      program: "path" in program ? { path: program.path } : { bundled: program.binary },
      args: [...program.args],
      env: { ...childEnv(), TMPDIR: dirs.tmp, TEMP: dirs.tmp, TMP: dirs.tmp },
      cwd: dirs.job,
      // The job directory read-only, `tmp/` read-write, each mapped file at its own mode (9a review H1).
      grants: [
        { path: dirs.job, mode: "read" },
        { path: dirs.tmp, mode: "readWrite" },
        ...files.map((f) => ({ path: mappedPath(dirs.job, f.name), mode: f.mode })),
      ],
      qos: row.priority === "interactive" ? "normal" : kind.qos,
      label: `${kind.kind} ${row.id}`,
    };
  }

  #onEvent(e: ProcEvent, proc: number, memoryBytes: number | undefined, grants: HelperGrant[]): void {
    this.#proc ??= proc;
    switch (e.type) {
      case "hello":
        return this.#onHello(e.hello, memoryBytes, grants);
      case "message":
        return this.#onMessage(e.msg);
      case "stderr":
        return this.#a.env.log("debug", `[job ${this.#a.row.id} ${this.#a.family.family}] ${e.line}`);
      case "exit":
        return void this.#onExit(e.info);
    }
  }

  #onHello(hello: Hello, memoryBytes: number | undefined, grants: HelperGrant[]): void {
    const { kind, family, control } = this.#a;
    if (this.#started || this.#fault !== undefined || this.#proc === undefined) return;
    if (control.requested !== undefined) {
      void this.#a.env.host.killGroup(this.#group, "cancel").catch(() => {});
      return;
    }
    const kinds = hello.kinds ?? [];
    if (hello.role !== "worker" || hello.family !== family.family || !kinds.some((k) => k.kind === kind.kind && k.version === kind.version)) {
      const error = finError("WORKER_PROTOCOL_MISMATCH", `the worker does not serve ${family.family}'s ${kind.kind} v${kind.version}`, {
        expected: { role: "worker", family: family.family, kind: kind.kind, version: kind.version },
        got: { role: hello.role, family: hello.family ?? null, kinds },
      });
      this.#failWith(error, "mismatch", "jobEnded");
      return;
    }
    this.#start(memoryBytes, grants);
  }

  #start(memoryBytes: number | undefined, helpers: HelperGrant[]): void {
    const { env, row, kind, dirs, files, params, extra } = this.#a;
    if (this.#proc === undefined) return;
    this.#started = true;
    this.#diagnostics = env.diagnostics();
    const job: StartJob = {
      id: row.id,
      kind: row.kind,
      kindVersion: row.kindVersion,
      params,
      attempt: row.attempts,
      ...(row.checkpoint === undefined ? {} : { checkpoint: row.checkpoint as NonNullable<StartJob["checkpoint"]> }),
      ...(extra === undefined ? {} : { extra: extra as NonNullable<StartJob["extra"]> }),
    };
    const start: HostMsg = {
      t: "start",
      job,
      limits: { aliveMs: env.aliveMs, ...(memoryBytes === undefined ? {} : { memoryBytes }) },
      files: files.map((f) => ({ name: f.name, path: mappedPath(dirs.job, f.name), mode: f.mode, ...(f.bytes === undefined ? {} : { bytes: f.bytes }) })),
      helpers,
      dirs: { job: dirs.job, tmp: dirs.tmp },
      diagnostics: this.#diagnostics,
    };
    env.host.send(this.#proc, start);
    env.host.watch(this.#proc, { job: row.id, stallMs: (kind.stallS ?? DEFAULT_STALL_S) * 1000, timeoutMs: kind.timeoutS * 1000 });
  }

  /** An item name over the bound is a protocol error (the job's group is ended). */
  #itemOk(item: string): boolean {
    if (item.length <= MAX_ITEM_CHARS) return true;
    this.#failWith(finError("WORKER_CRASHED", `the worker reported an item name of ${item.length} characters`, { protocol: "itemTooLong", limit: MAX_ITEM_CHARS }), "protocol", "jobEnded");
    return false;
  }

  #onMessage(msg: WorkerMsg): void {
    const { env, row, family, facts } = this.#a;
    if ((this.#answer !== undefined || this.#fault !== undefined) && msg.t !== "ready" && msg.t !== "call") {
      // Nothing counts after the answer (9a review M1): no item, checkpoint or progress is taken.
      env.log("warning", `job ${row.id}: ${msg.t} after the worker's answer is refused (INVALID_REQUEST)`);
      return;
    }
    switch (msg.t) {
      case "begin":
        if (this.#itemOk(msg.item)) facts.inFlight.add(msg.item);
        return;
      case "item": {
        if (!this.#itemOk(msg.item)) return;
        facts.inFlight.delete(msg.item);
        facts.done.push(msg.item);
        const outcome: ItemOutcome = msg.error !== undefined ? { error: jobErr(msg.error) } : msg.value === undefined ? {} : { value: msg.value };
        this.#items.add(msg.item, outcome);
        env.item(row.id, msg.item, outcome);
        return;
      }
      case "progress": {
        const p: JobProgress = {
          done: msg.done,
          unit: msg.unit,
          at: env.now(),
          ...(msg.total === undefined ? {} : { total: msg.total }),
          ...(msg.phase === undefined ? {} : { phase: msg.phase }),
        };
        env.progress(row.id, p);
        return;
      }
      case "checkpoint":
        this.#checkpoints.add(msg.data);
        return;
      case "result":
        this.#answered({ value: msg.value });
        return;
      case "error":
        for (const ie of msg.itemErrors ?? []) {
          if (!this.#itemOk(ie.item)) return;
          const outcome: ItemOutcome = { error: jobErr(ie.error) };
          facts.inFlight.delete(ie.item);
          this.#items.add(ie.item, outcome);
          env.item(row.id, ie.item, outcome);
        }
        this.#answered({ error: msg.error, retry: msg.error.retry ?? "never" });
        return;
      case "diag":
        if (this.#diagnostics && (family.diagKinds ?? []).includes(msg.kind)) {
          env.sink.forward({ family: family.family, job: msg.job ?? row.id }, msg.kind, msg.payload);
        } else env.diagDropped();
        return;
      case "call":
        this.#calls.call(msg.id, msg.method, msg.params);
        return;
      default:
        return; // `ready`, a repeated `hello`: nothing for a cold job
    }
  }

  #answered(a: Answer): void {
    const { env } = this.#a;
    if (this.#answer !== undefined || this.#fault !== undefined) return;
    this.#answer = a;
    this.#calls.refuse("after the job's answer");
    this.#helpers.answered();
    this.#cancelKill?.();
    if (this.#proc !== undefined) env.host.watch(this.#proc, null);
    // A cold worker exits once it answered; one that does not is ended with its group.
    this.#cancelExit = env.timers.after(EXIT_GRACE_MS, () => void env.host.killGroup(this.#group, "jobEnded").catch(() => {}));
  }

  async #onExit(info: JobHostExitInfo): Promise<void> {
    const { env, row, family } = this.#a;
    let outcome: Outcome | undefined;
    try {
      this.#cancelExit?.();
      this.#cancelKill?.();
      if (info.reason === "abandoned" && this.#proc !== undefined) env.abandoned(this.#proc, row.id);
      // Open host calls end before anything that reads the directory runs (9a review M1).
      await this.#calls.close();
      const verdict = exitVerdict(info, this.#answer !== undefined);
      const exit: WorkerExitEvent = {
        role: "worker",
        family: family.family,
        reason: verdict.reason,
        jobsServed: this.#started ? 1 : 0,
        lifetimeS: info.lifetimeMs / 1000,
        sandboxed: env.host.caps().sandboxed,
        ...(info.code === undefined ? {} : { exitCode: info.code }),
        ...(info.signal === undefined ? {} : { signal: info.signal }),
        ...(family.pluginId === undefined ? {} : { pluginId: family.pluginId }),
      };
      try {
        env.sink.workerExit(exit);
      } catch (e) {
        env.log("error", `job ${row.id}: worker.exit failed: ${toFinError(e).message}`);
      }
      this.#foreign = info.foreign ?? 0;
      if (this.#answer !== undefined && "value" in this.#answer) await this.#helpers.shutdown();
      // Every process of the group, the worker's leftovers and the helpers too, is gone before the outcome counts.
      await this.#endGroup();
      if (this.#foreign > 0) env.log("warning", `job ${row.id} (${family.family}) left ${this.#foreign} foreign process(es)`);
      outcome = this.#outcome(info, verdict.error);
    } catch (e) {
      outcome = { ok: false, error: finError("INTERNAL", `the attempt's end failed: ${toFinError(e).message}`), cause: "exit", exit: info };
    } finally {
      try {
        this.#items.flush();
        this.#checkpoints.flush();
      } catch (e) {
        env.log("error", `job ${row.id}: flushing items or the checkpoint failed: ${toFinError(e).message}`);
      }
      this.#settle(outcome ?? { ok: false, error: finError("INTERNAL", "the attempt ended without an outcome"), cause: "exit", exit: info });
    }
  }

  #outcome(info: JobHostExitInfo, exitError: FinError): Outcome {
    const { env, family } = this.#a;
    if (this.#fault !== undefined) return { ok: false, ...this.#fault, exit: info };
    const a = this.#answer;
    if (a !== undefined && "value" in a) {
      if (env.testHooks && this.#foreign > 0 && family.family !== "test") {
        return { ok: false, error: finError("WORKER_CRASHED", "the worker started processes", { foreign: this.#foreign }), cause: "foreign", exit: info };
      }
      return { ok: true, value: a.value };
    }
    if (a !== undefined) return { ok: false, error: jobErr(a.error), cause: "worker", retry: a.retry, exit: info };
    return { ok: false, error: exitError, cause: "exit", exit: info };
  }
}
