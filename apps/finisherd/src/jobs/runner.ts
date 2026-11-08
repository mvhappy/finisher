// One attempt of one cold job (spec §6.2–6.4, 5C-1 plan Task 9a): the job directory and its
// files → `prepare` (once per directory) → the worker in its own group (hello, kinds check,
// start) → its messages → result, error or exit → the group gone. Then, for a success, the
// result and `phase: hooks` are persisted and `onSucceeded` runs (Decision 40). The attempt's
// outcome carries what Task 10's isolation needs: the items acknowledged in this run and the
// items begun without an `item` (a set: a family may acknowledge a group of items after one
// reply, e.g. 5D's probe groups of up to 32).
import type { HostMsg, JobError as JobErrorMsg, Retry, WorkerMsg } from "@finisher/core-types";
import { FinError, type JobProgress, type JobView, type MappedFile } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { Cause, Effect, Exit, type Fiber } from "effect";
import { finError, toFinError } from "../errors.ts";
import { childEnv } from "../env.ts";
import type { JobEventSink } from "./events.ts";
import { CallRouter, type RunFork } from "./calls.ts";
import { checkPrepared, createFiles, createJobDir, type JobDirs, jobDirs, mappedPath } from "./files.ts";
import { type EngineHost, type JobHostExitInfo, type JobSpawnRequest, type ProcEvent, exitVerdict } from "./host.ts";
import { type FamilySpec, type ItemOutcome, type KindRegistry, type KindSpec, type MemoryRule, resolveOrFn } from "./kinds.ts";
import type { JobRow, JobStore } from "./store.ts";

export const ALIVE_MS = 5000;
export const DEFAULT_STALL_S = 600;
export const DEFAULT_HOOK_TIMEOUT_S = 60;
/** After a worker's answer, how long it may take to exit before its group is killed. */
export const EXIT_GRACE_MS = 2000;
export const ITEM_FLUSH_MS = 1000;
export const ITEM_FLUSH_ROWS = 256;
export const CHECKPOINT_EVERY_MS = 10_000;

type Log = (level: "debug" | "warning" | "error", message: string) => void;

/** What a runner needs from its engine. */
export interface RunnerEnv {
  readonly host: EngineHost;
  readonly store: JobStore;
  readonly registry: KindRegistry;
  readonly sink: JobEventSink;
  /** `<server-data>/jobs`. */
  readonly root: string;
  readonly totalMem: number;
  readonly now: () => number;
  readonly run: RunFork;
  readonly diagnostics: () => boolean;
  /** fin-node is a test-hooks build: a non-`test` family that leaves foreign processes fails (Decision 22). */
  readonly testHooks: boolean;
  readonly aliveMs: number;
  readonly log: Log;
  progress(job: string, p: JobProgress): void;
  item(job: string, item: string, outcome: ItemOutcome): void;
  diagDropped(): void;
}

/** Why an attempt failed, for the retry policy (Task 10 classifies it, Decision 31). */
export type FailureCause = "files" | "prepare" | "spawn" | "mismatch" | "worker" | "exit" | "foreign";

export type AttemptOutcome = AttemptFacts &
  (
    | { readonly ok: true; readonly value: unknown }
    | {
        readonly ok: false;
        readonly error: FinError;
        readonly cause: FailureCause;
        /** A worker `error`'s retry hint. */
        readonly retry?: Retry;
        readonly exit?: JobHostExitInfo;
      }
  );

export interface AttemptFacts {
  /** Items acknowledged (`item`) in this run, in order. */
  readonly done: ReadonlyArray<string>;
  /** Items begun and not acknowledged when the run ended, in `begin` order (a set). */
  readonly inFlight: ReadonlyArray<string>;
  readonly runMicros: number;
}

/** The memory rule of a job's group: the kind's, else the family's for its class (Decision 20). */
export function memoryRule(kind: KindSpec, family: FamilySpec, view: JobView): MemoryRule | undefined {
  if (kind.memory !== undefined) return resolveOrFn(kind.memory, view.params as never);
  return view.priority === "interactive" ? family.memory?.interactive : family.memory?.batch;
}

const jobErr = (e: JobErrorMsg): FinError =>
  new FinError({ code: e.code, message: e.message, details: e.details ?? {} });

/** Buffers item outcomes: one transaction per second or per 256 rows (Decision 26). */
class ItemWriter {
  #pending: Array<[string, ItemOutcome]> = [];
  #timer: ReturnType<typeof setTimeout> | undefined;
  readonly store: JobStore;
  readonly id: string;
  constructor(store: JobStore, id: string) {
    this.store = store;
    this.id = id;
  }
  add(item: string, outcome: ItemOutcome): void {
    this.#pending.push([item, outcome]);
    if (this.#pending.length >= ITEM_FLUSH_ROWS) this.flush();
    else this.#timer ??= setTimeout(() => this.flush(), ITEM_FLUSH_MS);
  }
  flush(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    const rows = this.#pending;
    this.#pending = [];
    this.store.writeItems(this.id, rows);
  }
}

/** Keeps the newest checkpoint and writes at most one per 10 s; `flush` writes what is held. */
class CheckpointWriter {
  #last = -Infinity;
  #held: { data: unknown } | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;
  readonly store: JobStore;
  readonly id: string;
  readonly now: () => number;
  constructor(store: JobStore, id: string, now: () => number) {
    this.store = store;
    this.id = id;
    this.now = now;
  }
  add(data: unknown): void {
    const wait = this.#last + CHECKPOINT_EVERY_MS - this.now();
    if (wait <= 0) {
      this.#write(data);
      return;
    }
    this.#held = { data };
    this.#timer ??= setTimeout(() => this.flush(), wait);
  }
  #write(data: unknown): void {
    this.#last = this.now();
    this.store.checkpoint(this.id, data);
  }
  flush(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    const held = this.#held;
    this.#held = undefined;
    if (held !== undefined) this.#write(held.data);
  }
}

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
  const exit = await awaitFiber(env.run(Effect.suspend(() => bounded)));
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

/**
 * One attempt of the running job `row` (already `running`, its `attempts` counting this one).
 * `params` are what the worker runs (a batch's remaining items, Task 10).
 */
export async function runAttempt(env: RunnerEnv, row: JobRow, kind: KindSpec, family: FamilySpec, params: unknown): Promise<AttemptOutcome> {
  const t0 = performance.now();
  const facts = { done: [] as string[], inFlight: new Set<string>() };
  const finish = (o: Record<string, unknown>): AttemptOutcome =>
    ({ ...o, done: facts.done, inFlight: [...facts.inFlight], runMicros: (performance.now() - t0) * 1000 }) as unknown as AttemptOutcome;
  const dirs = jobDirs(env.root, row.id);
  let files: ReadonlyArray<MappedFile>;
  let extra = row.extra;
  try {
    files = resolveOrFn(kind.files ?? [], params as never);
    const created = await createJobDir(dirs);
    await createFiles(dirs.job, files);
    if (created && kind.prepare !== undefined) {
      const prepare = kind.prepare;
      const out = await runHook(env, Effect.suspend(() => prepare(hookCtx(row, dirs, row.attempts))));
      if (!out.ok) return finish({ ok: false, error: out.error, cause: "prepare" });
      const value = out.value as { extra?: unknown } | undefined;
      if (value?.extra !== undefined) {
        extra = value.extra;
        env.store.transition(row.id, "running", { extra });
      }
    }
    await checkPrepared(dirs.job, files);
  } catch (e) {
    return finish({ ok: false, error: toFinError(e), cause: "files" });
  }
  return new Attempt(env, row, kind, family, params, dirs, files, extra, facts).run().then((o) => finish(o));
}

type Answer = { readonly value: unknown } | { readonly error: JobErrorMsg; readonly retry: Retry };

class Attempt {
  readonly #items: ItemWriter;
  readonly #checkpoints: CheckpointWriter;
  #group = -1;
  #proc: number | undefined;
  #started = false;
  #answer: Answer | undefined;
  #mismatch: FinError | undefined;
  #foreign = 0;
  #exitTimer: ReturnType<typeof setTimeout> | undefined;
  #calls: CallRouter | undefined;
  #diagnostics = false;
  #settle: (o: Record<string, unknown>) => void = () => {};

  readonly env: RunnerEnv;
  readonly row: JobRow;
  readonly kind: KindSpec;
  readonly family: FamilySpec;
  readonly params: unknown;
  readonly dirs: JobDirs;
  readonly files: ReadonlyArray<MappedFile>;
  readonly extra: unknown;
  readonly facts: { done: string[]; inFlight: Set<string> };

  constructor(
    env: RunnerEnv,
    row: JobRow,
    kind: KindSpec,
    family: FamilySpec,
    params: unknown,
    dirs: JobDirs,
    files: ReadonlyArray<MappedFile>,
    extra: unknown,
    facts: { done: string[]; inFlight: Set<string> },
  ) {
    this.env = env;
    this.row = row;
    this.kind = kind;
    this.family = family;
    this.params = params;
    this.dirs = dirs;
    this.files = files;
    this.extra = extra;
    this.facts = facts;
    this.#items = new ItemWriter(env.store, row.id);
    this.#checkpoints = new CheckpointWriter(env.store, row.id, env.now);
  }

  async run(): Promise<Record<string, unknown>> {
    const { env, row, kind, family } = this;
    const rule = memoryRule(kind, family, row);
    const memoryBytes = rule === undefined ? undefined : resolveMemoryRule(rule, env.totalMem);
    const qos = row.priority === "interactive" ? "normal" : kind.qos;
    this.#group = env.host.openGroup({ label: `${kind.kind} ${row.id}`, qos, ...(memoryBytes === undefined ? {} : { memoryBytes }) });
    const done = new Promise<Record<string, unknown>>((resolve) => {
      this.#settle = resolve;
    });
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
    try {
      // Held events are replayed inside `spawn`, so the proc id comes with each event too.
      this.#proc = await env.host.spawn(this.#group, this.#request(), (e, proc) => this.#onEvent(e, proc, memoryBytes), row.id);
    } catch (e) {
      env.host.closeGroup(this.#group);
      return { ok: false, error: toFinError(e), cause: "spawn" };
    }
    return done;
  }

  #request(): JobSpawnRequest {
    const { env, row, family } = this;
    const program = env.registry.program(family.family, row, "cold");
    const tmp = this.dirs.tmp;
    const envVars = { ...childEnv(), TMPDIR: tmp, TEMP: tmp, TMP: tmp };
    return {
      role: "worker",
      profile: family.profile,
      family: family.family,
      program: "path" in program ? { path: program.path } : { bundled: program.binary },
      args: [...program.args],
      env: envVars,
      cwd: this.dirs.job,
      grants: [
        { path: this.dirs.job, mode: "readWrite" },
        ...this.files.map((f) => ({ path: mappedPath(this.dirs.job, f.name), mode: f.mode })),
      ],
      qos: row.priority === "interactive" ? "normal" : this.kind.qos,
      label: `${this.kind.kind} ${row.id}`,
    } as JobSpawnRequest;
  }

  #send(msg: HostMsg): void {
    if (this.#proc !== undefined) this.env.host.send(this.#proc, msg);
  }

  #onEvent(e: ProcEvent, proc: number, memoryBytes: number | undefined): void {
    this.#proc ??= proc;
    switch (e.type) {
      case "hello":
        return this.#onHello(e.hello.kinds ?? [], memoryBytes);
      case "message":
        return this.#onMessage(e.msg);
      case "stderr":
        return this.env.log("debug", `[job ${this.row.id} ${this.family.family}] ${e.line}`);
      case "exit":
        return void this.#onExit(e.info);
    }
  }

  #onHello(kinds: ReadonlyArray<{ kind: string; version: number }>, memoryBytes: number | undefined): void {
    const { env, row, kind } = this;
    if (this.#started || this.#proc === undefined) return;
    if (!kinds.some((k) => k.kind === kind.kind && k.version === kind.version)) {
      this.#mismatch = finError("WORKER_PROTOCOL_MISMATCH", `the worker does not serve ${kind.kind} v${kind.version}`, {
        expected: { kind: kind.kind, version: kind.version },
        got: kinds,
      });
      void env.host.killGroup(this.#group, "jobEnded").catch(() => {});
      return;
    }
    this.#started = true;
    this.#diagnostics = env.diagnostics();
    const job: Record<string, unknown> = { id: row.id, kind: row.kind, kindVersion: row.kindVersion, params: this.params, attempt: row.attempts };
    if (row.checkpoint !== undefined) job.checkpoint = row.checkpoint;
    if (this.extra !== undefined) job.extra = this.extra;
    const start = {
      t: "start",
      job,
      limits: { aliveMs: env.aliveMs, ...(memoryBytes === undefined ? {} : { memoryBytes }) },
      files: this.files.map((f) => ({ name: f.name, path: mappedPath(this.dirs.job, f.name), mode: f.mode, ...(f.bytes === undefined ? {} : { bytes: f.bytes }) })),
      helpers: [],
      dirs: { job: this.dirs.job, tmp: this.dirs.tmp },
      diagnostics: this.#diagnostics,
    } as unknown as HostMsg;
    this.#send(start);
    env.host.watch(this.#proc, { job: row.id, stallMs: (kind.stallS ?? DEFAULT_STALL_S) * 1000, timeoutMs: kind.timeoutS * 1000 });
  }

  #onMessage(msg: WorkerMsg): void {
    const { env, row } = this;
    switch (msg.t) {
      case "begin":
        this.facts.inFlight.add(msg.item);
        return;
      case "item": {
        this.facts.inFlight.delete(msg.item);
        this.facts.done.push(msg.item);
        const outcome: ItemOutcome = msg.error !== undefined ? { error: jobErr(msg.error) } : msg.value === undefined ? {} : { value: msg.value };
        this.#items.add(msg.item, outcome);
        env.item(row.id, msg.item, outcome);
        return;
      }
      case "progress": {
        const p: Record<string, unknown> = { done: msg.done, unit: msg.unit, at: env.now() };
        if (msg.total !== undefined) p.total = msg.total;
        if (msg.phase !== undefined) p.phase = msg.phase;
        env.progress(row.id, p as unknown as JobProgress);
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
          const outcome: ItemOutcome = { error: jobErr(ie.error) };
          this.facts.inFlight.delete(ie.item);
          this.#items.add(ie.item, outcome);
          env.item(row.id, ie.item, outcome);
        }
        this.#answered({ error: msg.error, retry: msg.error.retry ?? "never" });
        return;
      case "diag":
        if (this.#diagnostics && (this.family.diagKinds ?? []).includes(msg.kind)) {
          env.sink.forward({ family: this.family.family, job: msg.job ?? row.id }, msg.kind, msg.payload);
        } else env.diagDropped();
        return;
      case "call":
        this.#calls?.call(msg.id, msg.method, msg.params);
        return;
      default:
        return; // `ready`, a repeated `hello`: nothing for a cold job
    }
  }

  #answered(a: Answer): void {
    if (this.#answer !== undefined) return;
    this.#answer = a;
    if (this.#proc !== undefined) this.env.host.watch(this.#proc, null);
    // A cold worker exits once it answered; one that does not is ended with its group.
    this.#exitTimer = setTimeout(() => void this.env.host.killGroup(this.#group, "jobEnded").catch(() => {}), EXIT_GRACE_MS);
  }

  async #onExit(info: JobHostExitInfo): Promise<void> {
    const { env, row, family } = this;
    if (this.#exitTimer !== undefined) clearTimeout(this.#exitTimer);
    this.#calls?.close();
    const verdict = exitVerdict(info, this.#answer !== undefined);
    env.sink.workerExit({
      role: "worker",
      family: family.family,
      reason: verdict.reason,
      jobsServed: this.#started ? 1 : 0,
      lifetimeS: info.lifetimeMs / 1000,
      sandboxed: env.host.caps().sandboxed,
      ...(info.code === undefined ? {} : { exitCode: info.code }),
      ...(info.signal === undefined ? {} : { signal: info.signal }),
      ...(family.pluginId === undefined ? {} : { pluginId: family.pluginId }),
    } as Parameters<JobEventSink["workerExit"]>[0]);
    this.#foreign = info.foreign ?? 0;
    // Every process of the group, the worker's leftovers too, is gone before the outcome counts.
    try {
      await env.host.killGroup(this.#group, "jobEnded");
    } catch (e) {
      env.log("warning", `job ${row.id}: ending its group failed: ${toFinError(e).message}`);
    }
    env.host.closeGroup(this.#group);
    this.#items.flush();
    this.#checkpoints.flush();
    if (this.#foreign > 0) env.log("warning", `job ${row.id} (${family.family}) left ${this.#foreign} foreign process(es)`);
    this.#settle(this.#outcome(info, verdict.error));
  }

  #outcome(info: JobHostExitInfo, exitError: FinError): Record<string, unknown> {
    if (this.#mismatch !== undefined) return { ok: false, error: this.#mismatch, cause: "mismatch", exit: info };
    const a = this.#answer;
    if (a !== undefined && "value" in a) {
      if (this.env.testHooks && this.#foreign > 0 && this.family.family !== "test") {
        return {
          ok: false,
          error: finError("WORKER_CRASHED", "the worker started processes", { foreign: this.#foreign }),
          cause: "foreign",
          exit: info,
        };
      }
      return { ok: true, value: a.value };
    }
    if (a !== undefined) return { ok: false, error: jobErr(a.error), cause: "worker", retry: a.retry, exit: info };
    return { ok: false, error: exitError, cause: "exit", exit: info };
  }
}
