// The engine's side of fin-node's `JobHost` (5C-1 plan Task 9a): routes the host's events by
// process id to the runner that spawned the process, and by group id to whoever waits for the
// group to empty, and turns an `ExitInfo` into a `worker.exit` reason and, when the exit ends a
// job, its error. A process's `spawn` has resolved, and the code after its `await` has run,
// before its first event (`spawned`) arrives (Task 6 review), so the listener is always in place;
// an event for an id nobody spawned is dropped (9a review L4).
import type {
  ExitReason,
  Hello,
  HostMsg,
  JobGroupSpec,
  JobHostCaps,
  JobHostEvent,
  JobHostExitInfo,
  JobHostKillReason,
  JobHostOptions,
  JobSpawnRequest,
  JobWatch,
  WorkerMsg,
} from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import { JobHost, type JobHostSendResult } from "@finisher/fin-node";
import { finError } from "../errors.ts";
import { isResourceFault } from "./isolation.ts";
import { currentSandboxStatus } from "../sandbox/status.ts";

export type { JobHostExitInfo, JobSpawnRequest } from "@finisher/core-types";

/** What `send` did. Never a cause: why a process is gone arrives with its `exit`. */
export type SendResult = JobHostSendResult;

/** The part of fin-node's `JobHost` the engine uses (tests pass a fake, 9a review). */
export interface JobHostLike {
  caps(): JobHostCaps;
  openGroup(spec: JobGroupSpec): number;
  spawn(group: number, req: JobSpawnRequest): Promise<number>;
  send(proc: number, msg: HostMsg): JobHostSendResult;
  watch(proc: number, w: JobWatch | null): void;
  closeStdin(proc: number): void;
  killGroup(group: number, reason: JobHostKillReason): Promise<void>;
  closeGroup(group: number): void;
  shutdown(graceMs: number): Promise<void>;
  readonly killProcForTest?: ((proc: number) => void) | undefined;
  readonly abandonForTest?: ((proc: number, holdMs?: number) => void) | undefined;
}

/** Starts a host whose events go to `onEvent` (default: fin-node's `JobHost.start`). */
export type JobHostFactory = (opts: JobHostOptions, onEvent: (e: JobHostEvent) => void) => Promise<JobHostLike>;

/** What a process's owner receives, in order: `hello`, messages and `stderr` lines, then `exit`. */
export type ProcEvent =
  | { readonly type: "hello"; readonly hello: Hello }
  | { readonly type: "message"; readonly msg: WorkerMsg; readonly memory?: number }
  | { readonly type: "stderr"; readonly line: string }
  | { readonly type: "exit"; readonly info: JobHostExitInfo };

/** Receives each event with its process id. */
export type ProcListener = (e: ProcEvent, proc: number) => void;

/** A live process, for `workers()`. */
export interface ProcView {
  readonly proc?: number;
  readonly group: number;
  readonly role: "worker" | "helper";
  readonly family: string;
  readonly job?: string;
  readonly pid?: number;
  readonly spawnedAt: number;
}

type Log = (level: "debug" | "warning" | "error", message: string) => void;

export class EngineHost {
  readonly #host: JobHostLike;
  readonly #caps: JobHostCaps;
  readonly #listeners = new Map<number, ProcListener>();
  readonly #groups = new Map<number, Array<() => void>>();
  readonly #live = new Map<object, { view: ProcView }>();
  readonly #byProc = new Map<number, object>();
  readonly #clock: () => number;
  #log: Log = () => {};
  /** A process reported `abandoned` has really exited (Decision 18). */
  onAbandonedExit: (proc: number) => void = () => {};

  private constructor(host: JobHostLike, clock: () => number, router: { deliver: (e: JobHostEvent) => void }) {
    this.#host = host;
    this.#caps = host.caps();
    this.#clock = clock;
    router.deliver = (e) => this.#deliver(e);
  }

  /** Starts the host (`factory`: fin-node's by default). `log` receives what the engine logs about processes. */
  static async start(opts: JobHostOptions, clock: () => number, log: Log, factory: JobHostFactory = (o, f) => JobHost.start(o, f)): Promise<EngineHost> {
    let early: JobHostEvent[] | undefined = [];
    const router = {
      deliver: (e: JobHostEvent): void => {
        early?.push(e);
      },
    };
    const host = await factory(opts, (e) => router.deliver(e));
    const engineHost = new EngineHost(host, clock, router);
    engineHost.#log = log;
    for (const e of early) router.deliver(e);
    early = undefined;
    return engineHost;
  }

  caps(): JobHostCaps {
    return this.#caps;
  }

  #deliver(e: JobHostEvent): void {
    switch (e.type) {
      case "spawned":
        return;
      case "groupEmpty": {
        const waiters = this.#groups.get(e.group);
        this.#groups.delete(e.group);
        for (const w of waiters ?? []) w();
        return;
      }
      case "abandonedExit":
        try {
          this.onAbandonedExit(e.proc);
        } catch (err) {
          this.#log("error", `the abandoned-exit handler threw: ${(err as Error).stack ?? String(err)}`);
        }
        return;
    }
    const listener = this.#listeners.get(e.proc);
    if (listener === undefined) {
      // Nobody spawned this id (or its exit was handled): dropped, never held (9a review L4).
      this.#log("debug", `job host: a ${e.type} event of unknown process ${e.proc} was dropped`);
      return;
    }
    const ev: ProcEvent =
      e.type === "hello"
        ? { type: "hello", hello: e.hello }
        : e.type === "message"
          ? e.memory === undefined
            ? { type: "message", msg: e.msg }
            : { type: "message", msg: e.msg, memory: e.memory }
          : e.type === "stderr"
            ? { type: "stderr", line: e.line }
            : { type: "exit", info: e.info };
    if (ev.type === "hello") this.#notePid(e.proc, ev.hello.pid);
    if (ev.type === "exit") this.#gone(e.proc);
    try {
      listener(ev, e.proc);
    } catch (err) {
      this.#log("error", `a job process listener threw: ${(err as Error).stack ?? String(err)}`);
    }
  }

  #notePid(proc: number, pid: number): void {
    const key = this.#byProc.get(proc);
    const entry = key === undefined ? undefined : this.#live.get(key);
    if (entry !== undefined) entry.view = { ...entry.view, pid };
  }

  #gone(proc: number): void {
    this.#listeners.delete(proc);
    const key = this.#byProc.get(proc);
    this.#byProc.delete(proc);
    if (key !== undefined) this.#live.delete(key);
  }

  openGroup(spec: JobGroupSpec): number {
    return this.#host.openGroup(spec);
  }

  /** Calls `f` once the group's last process exited. */
  onGroupEmpty(group: number, f: () => void): void {
    const list = this.#groups.get(group) ?? [];
    list.push(f);
    this.#groups.set(group, list);
  }

  /** Spawns a process whose events go to `listener`. It counts as live from this call until its `exit`. */
  async spawn(group: number, req: JobSpawnRequest, listener: ProcListener, job?: string): Promise<number> {
    const key = {};
    const view: ProcView = { group, role: req.role, family: req.family, spawnedAt: this.#clock(), ...(job === undefined ? {} : { job }) };
    this.#live.set(key, { view });
    let proc: number;
    try {
      proc = await this.#host.spawn(group, req);
    } catch (e) {
      this.#live.delete(key);
      throw e;
    }
    // Before any event of the process: the host delivers `spawned` only after this code ran.
    const entry = this.#live.get(key);
    if (entry !== undefined) entry.view = { ...entry.view, proc };
    this.#byProc.set(proc, key);
    this.#listeners.set(proc, listener);
    return proc;
  }

  /** Enqueues; `full` and `gone` say only that nothing was queued (the cause comes with `exit`). */
  send(proc: number, msg: HostMsg): SendResult {
    return this.#host.send(proc, msg);
  }

  watch(proc: number, w: JobWatch | null): void {
    this.#host.watch(proc, w);
  }

  closeStdin(proc: number): void {
    this.#host.closeStdin(proc);
  }

  /** Kills the group and resolves once its processes exited or were abandoned; a closed group is a no-op. */
  killGroup(group: number, reason: JobHostKillReason): Promise<void> {
    return this.#host.killGroup(group, reason);
  }

  /** Queues the group's release; after `shutdown` there is nothing left to release. */
  closeGroup(group: number): void {
    try {
      this.#host.closeGroup(group);
    } catch (e) {
      this.#log("debug", `closeGroup(${group}): ${(e as Error).message}`);
    }
  }

  shutdown(graceMs: number): Promise<void> {
    return this.#host.shutdown(graceMs);
  }

  /** The processes spawned and not yet exited (spawns in flight included). */
  processes(): ProcView[] {
    return [...this.#live.values()].map((e) => e.view);
  }

  /** The test-hooks build's direct process kill (a crash), or `undefined`. */
  get killProcForTest(): ((proc: number) => void) | undefined {
    return this.#host.killProcForTest;
  }

  /** The test-hooks build's abandonment: the process's exit is held `holdMs` once seen. */
  get abandonForTest(): ((proc: number, holdMs?: number) => void) | undefined {
    return this.#host.abandonForTest;
  }
}

/** A process exit as `worker.exit`'s reason and, when it ends a job, the job's error. */
export interface ExitVerdict {
  readonly reason: ExitReason;
  readonly error: FinError;
}

const KILL_REASONS: Partial<Record<JobHostKillReason, ExitReason>> = {
  cancel: "cancelled",
  recycle: "recycled",
  idleExpired: "idleExpired",
  hung: "hung",
};

/**
 * What the host lacks, from the line a worker printed before exit 78 (`… the sandbox needs
 * <need>`). Any other failure (a policy that would not build) names no path here: the full line
 * stays in the server log through the worker's stderr.
 */
export function sandboxNeed(stderrTail: string): string {
  const line = stderrTail.trim().split("\n").at(-1) ?? "";
  const m = /the sandbox needs (.+)$/.exec(line);
  return m?.[1] === undefined ? "a sandbox policy this host can apply" : m[1].slice(0, 200);
}

/**
 * The verdict for a process's exit. `ended`: the job already has its answer (`result` or
 * `error`), so a clean exit is `done`; otherwise the exit is the job's failure (Decision 31's
 * causes, classified by Task 10). A crash's `error.details` carry the exit code or signal and
 * `stderrTail` (stored in control.db only, never the journal).
 */
export function exitVerdict(info: JobHostExitInfo, ended: boolean): ExitVerdict {
  const facts: Record<string, unknown> = {};
  if (info.code !== undefined) facts.exitCode = info.code;
  if (info.signal !== undefined) facts.signal = info.signal;
  const tail = info.stderrTail === "" ? {} : { stderrTail: info.stderrTail };
  switch (info.reason) {
    case "exited":
      if (ended && info.code === 0) return { reason: "done", error: finError("WORKER_CRASHED", "the worker exited", facts) };
      if (!ended && isResourceFault(info)) {
        return { reason: "crashed", error: finError("WORKER_CRASHED", "a mapped file or disk failed under the worker", { ...facts, resourceFault: true, ...tail }) };
      }
      return {
        reason: "crashed",
        error: finError("WORKER_CRASHED", ended ? "the worker crashed after its answer" : "the worker exited without an answer", { ...facts, ...tail }),
      };
    case "resourceFault":
      return {
        reason: "crashed",
        error: finError("WORKER_CRASHED", "a mapped file or disk failed under the worker", { ...facts, resourceFault: true, ...tail }),
      };
    case "hung":
      return { reason: "hung", error: finError("WORKER_HUNG", "the worker stopped answering", facts) };
    case "stalled":
      return { reason: "stalled", error: finError("JOB_STALLED", "the job made no progress for too long", facts) };
    case "timedOut":
      return { reason: "killed", error: finError("JOB_TIMEOUT", "the job ran past its time limit", facts) };
    case "oom":
      return {
        reason: "oom",
        error: finError("WORKER_OOM", "the job's processes passed their memory limit", {
          ...facts,
          ...(info.memoryPeak === undefined ? {} : { used: info.memoryPeak }),
        }),
      };
    case "noHello":
      return { reason: "spawnFailed", error: finError("WORKER_UNAVAILABLE", "the worker did not start in time", { reason: "noHello" }) };
    case "abandoned":
      return { reason: "abandoned", error: finError("WORKER_CRASHED", "the worker did not exit when killed", { ...facts, abandoned: true }) };
    case "killed":
      return {
        reason: KILL_REASONS[info.kill] ?? "killed",
        error:
          info.kill === "cancel"
            ? finError("JOB_CANCELLED", "the job was cancelled")
            : info.kill === "hung"
              ? finError("WORKER_HUNG", "the worker stopped reading its input", facts)
              : finError("WORKER_CRASHED", `the worker was killed (${info.kill})`, { ...facts, killed: info.kill }),
      };
    case "protocolError":
      return { reason: "protocolError", error: finError("WORKER_CRASHED", `the worker broke the protocol: ${info.what}`, { protocol: info.what, ...tail }) };
    case "spawnFailed":
      // Exit 78 before `hello`: the requested self-sandbox could not be applied (fail closed,
      // plan 7C). Its stderr line names what the host lacks.
      if (info.what === "selfSandbox") {
        return {
          reason: "spawnFailed",
          error: finError("SANDBOX_UNAVAILABLE", "the worker's sandbox could not be applied, so it did not start", {
            profile: "worker",
            os: currentSandboxStatus().os,
            need: sandboxNeed(info.stderrTail),
          }),
        };
      }
      return { reason: "spawnFailed", error: finError("WORKER_UNAVAILABLE", `the worker could not start: ${info.what}`, { reason: "spawnFailed", what: info.what }) };
    case "protocolMismatch":
      return {
        reason: "protocolError",
        error: finError("WORKER_PROTOCOL_MISMATCH", "the worker speaks another protocol version", { expected: info.expected, got: info.got }),
      };
  }
}
