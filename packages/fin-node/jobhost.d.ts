// Types of `jobhost.js`: fin-jobhost's host and the job params helpers (re-exported by `index.d.ts`).
import type {
  AclGrant,
  HostMsg,
  JobGroupSpec,
  JobHostCaps,
  JobHostEvent,
  JobHostKillReason,
  JobHostOptions,
  JobMemoryRule,
  JobSpawnRequest,
  JobWatch,
} from "@finisher/core-types";

// The job host's JSON shapes are fin-jobhost's `wire` types, generated into core-types.
export type {
  JobGroupSpec,
  JobHostCaps,
  JobHostEvent,
  JobHostExitInfo,
  JobHostKillReason,
  JobHostOptions,
  JobHostQos,
  JobMemoryRule,
  JobSpawnRequest,
  JobWatch,
} from "@finisher/core-types";

/** What `JobHost.send` did: queued; `full` (the process is not reading its stdin and is being
 * killed as hung); `gone` (it exited, its stdin is closed, or the id is unknown). Neither names
 * how the process ends: that comes with its `exit` event. */
export type JobHostSendResult = "ok" | "full" | "gone";

/**
 * `fin-jobhost`'s host (spawning, kill trees, framing, timers). Nothing waits on the JS thread:
 * `start`, `spawn`, `killGroup` and `shutdown` settle from the host's threads; `openGroup`
 * allocates; `send` enqueues. Errors are `FinNodeError`s: `WORKER_UNAVAILABLE`
 * (`details.reason`) when no process could be started, `JOBHOST_STATE` (`details.reason`:
 * `unknownGroup`, `groupClosed`, `shutDown`) for a group or host the caller should know is
 * gone, `INVALID_REQUEST` (`details.field`) for an argument it cannot read, `INTERNAL` for an
 * answer the host lost (`details.reason: "unanswered"`).
 *
 * Events: per process `spawned`, `hello`, its messages and `stderr` lines, then `exit`
 * (`JobHostEvent`). A process's `spawn` has resolved, and the code after an `await` of it has
 * run, before its `spawned` reaches `onEvent`.
 *
 * The event callback is weak: a running host or process does not keep Node alive (pending
 * `start`, `spawn`, `killGroup` and `shutdown` promises do). Keep the process alive by other
 * means (a server, a timer) while jobs run.
 */
export declare class JobHost {
  private constructor();
  /** Resolves once the host runs (its Unix watchdog or Windows spawn broker started). */
  static start(opts: JobHostOptions, onEvent: (e: JobHostEvent) => void): Promise<JobHost>;
  caps(): JobHostCaps;
  /**
   * The ACEs a spawn of `req` needs on Windows (plan 7C Decision 18; pure, every OS answers):
   * `[]` where its profile passes; a request the spawner would refuse throws
   * `SANDBOX_UNAVAILABLE`. `finisherd` grants them through the ACL broker before the spawn.
   */
  aclGrants(req: JobSpawnRequest): AclGrant[];
  /** Allocates a group id; its OS objects are made at its first spawn. */
  openGroup(spec: JobGroupSpec): number;
  /** Resolves to the process id before any event of the process reaches `onEvent`. */
  spawn(group: number, req: JobSpawnRequest): Promise<number>;
  /** Enqueues `msg`; see `JobHostSendResult`. */
  send(proc: number, msg: HostMsg): JobHostSendResult;
  watch(proc: number, w: JobWatch | null): void;
  closeStdin(proc: number): void;
  killGroup(group: number, reason: JobHostKillReason): Promise<void>;
  /** Queues the group's release (later spawns fail; live processes are killed). */
  closeGroup(group: number): void;
  shutdown(graceMs: number): Promise<void>;
  /** @internal Test builds only: kills one process as a crash would. */
  readonly killProcForTest: ((proc: number) => void) | undefined;
  /** @internal Test builds only: its exit is held `holdMs` (60 s) once seen, so a kill abandons it. */
  readonly abandonForTest: ((proc: number, holdMs?: number) => void) | undefined;
  /** @internal Test builds only: events wait `ms` before they reach JS (a slow event loop). */
  readonly pauseEventsForTest: ((ms: number) => void) | undefined;
}

/**
 * Decodes `params` with the kind's Rust params type; errors name the field. Params over 512 KiB
 * of JSON are refused (`ok: false`); a `version` that is not a `u32` throws `INVALID_REQUEST`.
 */
export declare function validateJobParams(
  kind: string,
  version: number,
  params: unknown,
): { ok: true; params: unknown } | { ok: false; errors: string[] };
/** The kinds and versions `validateJobParams` knows (release builds: none yet). */
export declare function jobKindVersions(): Array<{ kind: string; version: number }>;
/** A group memory rule (Decision 20) in bytes for `totalBytes` of RAM; `undefined` without a bound. */
export declare function resolveMemoryRule(rule: JobMemoryRule, totalBytes: number): number | undefined;
/** `fin-workers[.exe]` in `dir` (default: the directory the fin-node binary was loaded from). */
export declare function workersBinaryPath(dir?: string): string;
