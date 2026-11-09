// What a cold and a warm attempt share (spec §6.2–6.4, 5C-1 plan Tasks 9a, 9b, 11): the worker's
// `start`, its messages (items, progress, checkpoints, diagnostics, host calls, the answer),
// the stop (cancel, pause, close: `cancel` to a started worker, its processes killed after the
// grace), the job's helpers, and the attempt's facts for the retry policy. A cold attempt
// (`runner.ts`) spawns its worker and ends with its exit; a warm one (`warm-run.ts`) borrows a
// ready worker of the pool and ends with its `ready`.
import type { CancelReason, Hello, HelperGrant, HostMsg, JobError as JobErrorMsg, Retry, StartJob, WorkerMsg } from "@finisher/core-types";
import { FinError, type JobProgress, type MappedFile } from "@finisher/contracts";
import { finError, toFinError } from "../errors.ts";
import { CallRouter } from "./calls.ts";
import type { WorkerExitEvent } from "./events.ts";
import { type JobDirs, mappedPath } from "./files.ts";
import { Helpers } from "./helpers.ts";
import type { ExitVerdict, JobHostExitInfo } from "./host.ts";
import { exitPluginId, type FamilySpec, type ItemOutcome, type KindSpec } from "./kinds.ts";
import type { RunControl, StopReason } from "./run-control.ts";
import type { FailureCause, RunnerEnv } from "./runner.ts";
import type { JobRow } from "./store.ts";
import { CheckpointWriter, ItemWriter } from "./writers.ts";

export const DEFAULT_STALL_S = 600;
/** The longest item name a worker may report (a longer one is a protocol error). */
export const MAX_ITEM_CHARS = 1024;

export type Outcome =
  | { readonly ok: true; readonly value: unknown }
  | {
      readonly ok: false;
      readonly error: FinError;
      readonly cause: FailureCause;
      /** A worker `error`'s retry hint. */
      readonly retry?: Retry;
      readonly exit?: JobHostExitInfo;
    };

export type Answer = { readonly value: unknown } | { readonly error: JobErrorMsg; readonly retry: Retry };

/** What the attempt learned as it ran. */
export interface Facts {
  done: string[];
  inFlight: Set<string>;
  helpers: number;
  stopSent: boolean;
}

export interface AttemptSpec {
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

export type KillReason = "helperFailed" | "jobEnded" | "cancel";

export const jobErr = (e: JobErrorMsg): FinError => new FinError({ code: e.code, message: e.message, details: e.details ?? {} });

export const stopped = (): FinError => finError("JOB_CANCELLED", "the job was cancelled");

/** `WORKER_PROTOCOL_MISMATCH` when the worker's `hello` does not serve the kind, else `undefined`. */
export function helloMismatch(hello: Hello, family: FamilySpec, kind: KindSpec, warm: boolean): FinError | undefined {
  const kinds = hello.kinds ?? [];
  const serves = hello.role === "worker" && hello.family === family.family && kinds.some((k) => k.kind === kind.kind && k.version === kind.version);
  if (serves && (!warm || hello.warm === true)) return undefined;
  return finError("WORKER_PROTOCOL_MISMATCH", `the ${warm ? "warm " : ""}worker does not serve ${family.family}'s ${kind.kind} v${kind.version}`, {
    expected: { role: "worker", family: family.family, kind: kind.kind, version: kind.version, ...(warm ? { warm: true } : {}) },
    got: { role: hello.role, family: hello.family ?? null, kinds, ...(warm ? { warm: hello.warm ?? false } : {}) },
  });
}

/**
 * Sends a `worker.exit` (a sink that throws is logged). `plugin`: the plugin it names, from
 * the job's or the warm worker's `via` (sp8b Decision 3b; the family's own `pluginId` wins).
 */
export function sendWorkerExit(
  env: RunnerEnv,
  family: FamilySpec,
  info: JobHostExitInfo,
  reason: WorkerExitEvent["reason"],
  jobsServed: number,
  job: string,
  plugin: string | undefined = family.pluginId,
): void {
  const exit: WorkerExitEvent = {
    role: "worker",
    family: family.family,
    reason,
    jobsServed,
    lifetimeS: info.lifetimeMs / 1000,
    sandboxed: env.host.caps().sandboxed,
    ...(info.code === undefined ? {} : { exitCode: info.code }),
    ...(info.signal === undefined ? {} : { signal: info.signal }),
    ...(plugin === undefined ? {} : { pluginId: plugin }),
  };
  try {
    env.sink.workerExit(exit);
  } catch (e) {
    env.log("error", `${job}: worker.exit failed: ${toFinError(e).message}`);
  }
}

export abstract class AttemptBase {
  protected readonly a: AttemptSpec;
  protected readonly items: ItemWriter;
  protected readonly checkpoints: CheckpointWriter;
  protected readonly calls: CallRouter;
  protected readonly helpers: Helpers;
  protected proc: number | undefined;
  protected started = false;
  protected answer: Answer | undefined;
  /** Why the attempt failed before or beside the worker's own outcome (the first one counts). */
  protected fault: { error: FinError; cause: FailureCause } | undefined;
  protected cancelKill: (() => void) | undefined;
  protected settle: (o: Outcome) => void = () => {};
  protected unbind: () => void = () => {};
  #diagnostics = false;

  /** `helperGroup`: the group the job's helpers are spawned in. */
  constructor(a: AttemptSpec, helperGroup: () => number) {
    this.a = a;
    const { env, row, family } = a;
    this.items = new ItemWriter(env, row.id);
    this.checkpoints = new CheckpointWriter(env, row.id);
    this.calls = new CallRouter(
      family.onCall,
      () => ({
        job: env.store.get(row.id) ?? row,
        role: "worker",
        owner: row.owner,
        ...(row.via === undefined ? {} : { via: row.via }),
        ...(row.rights === undefined ? {} : { rights: row.rights }),
      }),
      (msg) => (this.proc === undefined ? "gone" : env.host.send(this.proc, msg)),
      env.run,
    );
    const qos = row.priority === "interactive" ? "normal" : a.kind.qos;
    this.helpers = new Helpers(env, row, a.dirs, a.files, { group: helperGroup, qos, fail: (e) => this.failWith(e, "helper", "helperFailed") });
  }

  /** Kills the job's processes (cold: its group; warm: its helper group and the worker). */
  protected abstract kill(reason: KillReason): void;

  /** The worker answered: what ends the attempt now (cold: its exit; warm: its `ready`). */
  protected abstract afterAnswer(): void;

  /** A `ready` from the worker (only a warm worker sends one). */
  protected onReady(_memory: number | undefined): void {}

  /** Records the attempt's failure (the first one counts) and kills the job's processes. */
  protected failWith(error: FinError, cause: FailureCause, kill: KillReason): void {
    if (this.fault !== undefined || this.answer !== undefined) return;
    this.fault = { error, cause };
    this.kill(kill);
  }

  /** Cancel or pause (Decisions 23, 37): `cancel` to a started worker, its processes killed after the grace. */
  protected stop(reason: StopReason, graceMs: number): void {
    const { env, row } = this.a;
    if (this.answer !== undefined) return; // it answered: the attempt ends on its own
    this.a.facts.stopSent = true;
    if (this.started && this.proc !== undefined) {
      const wire: CancelReason = reason === "cancel" ? "cancel" : "pause";
      env.host.send(this.proc, { t: "cancel", job: row.id, reason: wire });
      this.cancelKill?.();
      this.cancelKill = env.timers.after(graceMs, () => this.kill("cancel"));
    } else this.kill("cancel");
  }

  /** Writes the `start` and arms the job's stall and timeout timers. */
  protected sendStart(memoryBytes: number | undefined, helpers: HelperGrant[]): void {
    const { env, row, kind, dirs, files, params, extra } = this.a;
    if (this.proc === undefined) return;
    this.started = true;
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
    env.host.send(this.proc, start);
    env.host.watch(this.proc, { job: row.id, stallMs: (kind.stallS ?? DEFAULT_STALL_S) * 1000, timeoutMs: kind.timeoutS * 1000 });
  }

  /** An item name over the bound is a protocol error (the job's processes are ended). */
  #itemOk(item: string): boolean {
    if (item.length <= MAX_ITEM_CHARS) return true;
    this.failWith(finError("WORKER_CRASHED", `the worker reported an item name of ${item.length} characters`, { protocol: "itemTooLong", limit: MAX_ITEM_CHARS }), "protocol", "jobEnded");
    return false;
  }

  protected onMessage(msg: WorkerMsg, memory?: number): void {
    const { env, row, family, facts } = this.a;
    if ((this.answer !== undefined || this.fault !== undefined) && msg.t !== "ready" && msg.t !== "call") {
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
        this.items.add(msg.item, outcome);
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
        this.checkpoints.add(msg.data);
        return;
      case "result":
        this.answered({ value: msg.value });
        return;
      case "error":
        for (const ie of msg.itemErrors ?? []) {
          if (!this.#itemOk(ie.item)) return;
          const outcome: ItemOutcome = { error: jobErr(ie.error) };
          facts.inFlight.delete(ie.item);
          this.items.add(ie.item, outcome);
          env.item(row.id, ie.item, outcome);
        }
        this.answered({ error: msg.error, retry: msg.error.retry ?? "never" });
        return;
      case "diag":
        if (this.#diagnostics && (family.diagKinds ?? []).includes(msg.kind)) {
          env.sink.forward({ family: family.family, job: msg.job ?? row.id }, msg.kind, msg.payload);
        } else env.diagDropped();
        return;
      case "call":
        this.calls.call(msg.id, msg.method, msg.params);
        return;
      case "ready":
        this.onReady(memory);
        return;
      default:
        return; // a repeated `hello`, `alive`
    }
  }

  #answered(a: Answer): void {
    const { env } = this.a;
    this.answer = a;
    this.calls.refuse("after the job's answer");
    this.helpers.answered();
    this.cancelKill?.();
    if (this.proc !== undefined) env.host.watch(this.proc, null);
  }

  protected answered(a: Answer): void {
    if (this.answer !== undefined || this.fault !== undefined) return;
    this.#answered(a);
    this.afterAnswer();
  }

  /** The outcome: a fault first, then the worker's answer, else the exit's error. */
  protected outcomeOf(info: JobHostExitInfo | undefined, exitError: FinError | undefined): Outcome {
    const exit = info === undefined ? {} : { exit: info };
    if (this.fault !== undefined) return { ok: false, ...this.fault, ...exit };
    const a = this.answer;
    if (a !== undefined && "value" in a) return { ok: true, value: a.value };
    if (a !== undefined) return { ok: false, error: jobErr(a.error), cause: "worker", retry: a.retry, ...exit };
    return { ok: false, error: exitError ?? finError("INTERNAL", "the attempt ended without an outcome"), cause: "exit", ...exit };
  }

  /** The items and the checkpoint held, written (a failure is logged). */
  protected flush(): void {
    try {
      this.items.flush();
      this.checkpoints.flush();
    } catch (e) {
      this.a.env.log("error", `job ${this.a.row.id}: flushing items or the checkpoint failed: ${toFinError(e).message}`);
    }
  }

  /** The verdict's exit, as a `worker.exit` of this attempt's worker. */
  protected workerExit(info: JobHostExitInfo, verdict: ExitVerdict, jobsServed: number): void {
    sendWorkerExit(this.a.env, this.a.family, info, verdict.reason, jobsServed, `job ${this.a.row.id}`, exitPluginId(this.a.family, this.a.row.via));
  }
}
