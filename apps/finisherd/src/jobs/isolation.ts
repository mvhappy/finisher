// Retries and the isolation of a crashing batch (spec §6.5, 5C-1 plan Decisions 31, 32), as pure
// functions: `classify` names a failed attempt's class in Decision 31's order, `nextStep` says
// what follows it (a retry with its delay, isolation, or the end), and the isolation state
// machine finds the items that crash a batch. Items acknowledged with `item` are never run
// again. After a crash, the items in flight (`begin` without `item`, a set: 5D acknowledges a
// probe group of up to 32 items after one reply) run first: one item alone, more bisected;
// then the rest. A run of one item that crashes poisons it (`POISON_ITEM`). With nothing in
// flight the remainder is halved (a remainder of one runs alone next). Crashing runs are budgeted: at most `2⌈log₂ n⌉ + 2p`
// (n items, p poisoned so far), capped at `2n`; past it the job fails, a crash no single item
// explains.
import type { FinError } from "@finisher/contracts";
import type { Retry } from "@finisher/core-types";
import { finError } from "../errors.ts";
import type { JobHostExitInfo } from "./host.ts";
import type { FailureCause } from "./runner.ts";

export type FailureClass = "resource" | "crash" | "oom" | "oomWarm" | "transient" | "deterministic" | "fresh" | "mismatch" | "hook" | "cancel";

export const BACKOFF_BASE_MS = 5000;
export const BACKOFF_MAX_MS = 5 * 60_000;

/** What `classify` reads of a failed attempt (or of a failed `onSucceeded`: `hook`). */
export interface FailureFacts {
  readonly cause?: FailureCause;
  readonly error: FinError;
  /** A worker `error`'s retry hint. */
  readonly retry?: Retry;
  readonly exit?: JobHostExitInfo;
  /** The worker was a reused warm one (Task 11). */
  readonly warmReused?: boolean;
  /** The kind's `resourceHealthy` answer, asked before any `POISON_ITEM` verdict. */
  readonly healthy?: boolean;
  /** The failure is the kind's `onSucceeded` hook's. */
  readonly hook?: boolean;
}

/** Codes of an exit that are resource problems: backoff, never bisected. */
const TRANSIENT_EXITS = new Set(["WORKER_HUNG", "JOB_STALLED", "JOB_TIMEOUT", "WORKER_UNAVAILABLE"]);

/** `EXCEPTION_IN_PAGE_ERROR` (0xC0000006), as an unsigned or a signed exit code. */
const IN_PAGE_ERROR = [0xc0000006, 0xc0000006 - 2 ** 32];

/**
 * A resource fault (Decision 31): a mapped file or disk failed under the process. The host's
 * own `resourceFault`, `SIGBUS` (Unix) or `EXCEPTION_IN_PAGE_ERROR` (Windows).
 */
export function isResourceFault(info: JobHostExitInfo): boolean {
  return info.reason === "resourceFault" || (info.reason === "exited" && (info.signal === "bus" || IN_PAGE_ERROR.includes(info.code ?? 0)));
}

const withDetails = (e: FinError, more: Record<string, unknown>): FinError =>
  finError(e.code, e.message, { ...(e.details as Record<string, unknown>), ...more }, e.fix);

/**
 * Decision 31's class of a failure, decided in the table's order: a resource fault (`SIGBUS`
 * or `EXCEPTION_IN_PAGE_ERROR`; an abandoned process, stuck in the kernel; any crash when the
 * kind's `resourceHealthy` answered false), a crash, `WORKER_OOM` (cold, or on a reused warm
 * worker), the transient exits and failures, a worker's own hint, the never-retried ones, cancel.
 */
export function classify(f: FailureFacts): { cls: FailureClass; error: FinError } {
  const e = f.error;
  const is = (cls: FailureClass, error: FinError = e) => ({ cls, error });
  if (f.hook === true || f.cause === "prepare") return is("hook");
  if (f.cause === "stopped" || e.code === "JOB_CANCELLED") return is("cancel");
  if (f.cause === "exit" || f.cause === "protocol") {
    const crash = f.cause === "protocol" || e.code === "WORKER_CRASHED";
    if ((f.exit !== undefined && isResourceFault(f.exit)) || f.exit?.reason === "abandoned") return is("resource");
    if (crash && f.healthy === false) return is("resource", withDetails(e, { resourceHealthy: false }));
    if (crash) return is("crash");
    if (e.code === "WORKER_OOM") return is(f.warmReused === true ? "oomWarm" : "oom");
    if (TRANSIENT_EXITS.has(e.code)) return is("transient");
    if (e.code === "WORKER_PROTOCOL_MISMATCH") return is("mismatch");
    return is("deterministic");
  }
  switch (f.cause) {
    // A sandbox spawner's refusal (an enforced profile it cannot apply, a grant it cannot
    // place; plan 7C Task 7) is the same on every try, as the self-sandbox's exit 78 is.
    case "spawn":
      return is(e.code === "SANDBOX_UNAVAILABLE" ? "deterministic" : "transient");
    case "helper":
      return is("transient");
    case "worker":
      return is(f.retry === "backoff" ? "transient" : f.retry === "fresh" ? "fresh" : "deterministic");
    case "mismatch":
      return is("mismatch");
    // A file the kind's `prepare` left wrong is the kind's bug; an I/O failure making the directory is not.
    case "files":
      return is((e.details as { problem?: unknown } | undefined)?.problem === undefined ? "transient" : "deterministic");
    default: // tampered, foreign
      return is("deterministic");
  }
}

/** `5 s × 2ⁿ` for n attempts made so far (10 s, 20 s, 40 s …), at most 5 minutes. */
export function backoffMs(attemptsSoFar: number): number {
  return Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** Math.max(0, attemptsSoFar));
}

export type Step = { readonly do: "retry"; readonly delayMs: number } | { readonly do: "isolate" } | { readonly do: "fail" };

export interface StepCtx {
  /** Attempts made, the failed one included. */
  readonly attempts: number;
  readonly maxAttempts: number;
  readonly interactive: boolean;
  /** A cold batch job (only those are isolated). */
  readonly batch: boolean;
  /** Items in flight when the run ended. */
  readonly inFlight: number;
  /** The job already had its one retry on a fresh process (`fresh`, `oomWarm`). */
  readonly freshUsed: boolean;
}

const FAIL: Step = { do: "fail" };

/**
 * What follows a failure of class `cls` (Decision 31): interactive jobs retry at once and are
 * never isolated; `fresh` and `oomWarm` retry once, at once; resource and transient failures,
 * and crashes of jobs that are not cold batches, retry with backoff; a crash of a cold batch,
 * or its `WORKER_OOM` with an item in flight, is isolated (no attempt spent); the rest fail.
 */
export function nextStep(cls: FailureClass, c: StepCtx): Step {
  const retry = (delayMs: number): Step => (c.attempts < c.maxAttempts ? { do: "retry", delayMs } : FAIL);
  const later = (): Step => retry(c.interactive ? 0 : backoffMs(c.attempts));
  switch (cls) {
    case "cancel":
    case "mismatch":
    case "hook":
    case "deterministic":
      return FAIL;
    case "fresh":
    case "oomWarm":
      return c.freshUsed ? FAIL : retry(0);
    case "resource":
    case "transient":
      return later();
    case "oom":
      if (c.inFlight === 0) return FAIL;
      return c.batch && !c.interactive ? { do: "isolate" } : later();
    case "crash":
      return c.batch && !c.interactive ? { do: "isolate" } : later();
  }
}

/** Decision 32's state (persisted in `jobs.isolation`). */
export interface IsolationState {
  /** The job's item count. */
  n: number;
  /** Items not yet resolved (the runs of `queue`, in order). */
  pending: string[];
  /** The item now run alone because it was in flight at a crash. */
  inFlightAlone?: string;
  /** The runs to make, the next first. */
  queue: string[][];
  poisoned: string[];
  crashRuns: number;
}

/** `min(2n, 2⌈log₂ n⌉ + 2p)`. */
export function budget(s: IsolationState): number {
  const log = s.n <= 1 ? 0 : Math.ceil(Math.log2(s.n));
  return Math.min(2 * s.n, 2 * log + 2 * s.poisoned.length);
}

/** Isolation of a batch whose run of `items` crashed (`n`: the job's item count, default theirs). */
export function startIsolation(items: ReadonlyArray<string>, n = items.length): IsolationState {
  return { n, pending: [...items], queue: items.length === 0 ? [] : [[...items]], poisoned: [], crashRuns: 0 };
}

const halves = (items: ReadonlyArray<string>): string[][] => {
  const mid = Math.ceil(items.length / 2);
  return [items.slice(0, mid), items.slice(mid)].filter((h) => h.length > 0);
};

function settle(s: IsolationState, queue: string[][], extra: Partial<IsolationState>): IsolationState {
  const runs = queue.filter((r) => r.length > 0);
  const { inFlightAlone: _drop, ...rest } = s;
  return { ...rest, ...extra, queue: runs, pending: runs.flat() };
}

/**
 * The state after the run of `items` (the head of the queue): `done` were acknowledged,
 * `inFlight` begun without `item`. Without a crash its items are resolved. After a crash its
 * unresolved items go first: a run of one item poisons it; one item left runs alone next; the
 * in-flight ones run first (one alone, several in halves), then the rest; with none in flight
 * the remainder is halved. Answers `exhausted` once the crashing runs pass the budget.
 */
export function afterRun(
  s: IsolationState,
  run: { readonly items: ReadonlyArray<string>; readonly done: ReadonlyArray<string>; readonly inFlight?: ReadonlyArray<string>; readonly crashed: boolean },
): IsolationState | "exhausted" {
  const resolved = new Set(run.done);
  const ran = new Set(run.items);
  // Every item of this run is resolved now or goes to the front again: none stays in the rest of the queue.
  const tail = s.queue.slice(1).map((r) => r.filter((i) => !resolved.has(i) && !ran.has(i)));
  if (!run.crashed) return settle(s, tail, {});
  const left = run.items.filter((i) => !resolved.has(i));
  const leftSet = new Set(left);
  const inFlight = [...new Set(run.inFlight ?? [])].filter((i) => leftSet.has(i));
  const crashRuns = s.crashRuns + 1;
  let poisoned = s.poisoned;
  let front: string[][] = [];
  let alone: string | undefined;
  if (left.length === 1 && run.items.length === 1) poisoned = [...poisoned, left[0] as string];
  else if (left.length === 1) {
    // A remainder of one is run alone next (Decision 32).
    front = [left];
    alone = left[0];
  } else if (left.length > 1 && inFlight.length > 0) {
    const flying = new Set(inFlight);
    front = [...(inFlight.length === 1 ? [inFlight] : halves(inFlight)), left.filter((i) => !flying.has(i))];
    if (inFlight.length === 1) alone = inFlight[0];
  } else if (left.length > 1) front = halves(left);
  const next = settle(s, [...front, ...tail], { poisoned, crashRuns, ...(alone === undefined ? {} : { inFlightAlone: alone }) });
  return crashRuns > budget(next) ? "exhausted" : next;
}

/** The next run's items; `undefined` when nothing is left. */
export function nextRun(s: IsolationState): ReadonlyArray<string> | undefined {
  return s.queue[0];
}

/** The state with the items `done` (acknowledged since it was saved) dropped. */
export function withoutDone(s: IsolationState, done: ReadonlySet<string>): IsolationState {
  return settle(s, s.queue.map((r) => r.filter((i) => !done.has(i))), s.inFlightAlone === undefined ? {} : { inFlightAlone: s.inFlightAlone });
}
