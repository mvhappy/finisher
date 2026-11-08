// Host calls (spec §6.1, 5C-1 plan Decision 5a): a process's `call {id, method, params}` goes
// to its family's `onCall` (a helper's to its `HelperProgram.onCall`) with a `CallCtx`; the
// Effect's value or `FinError` goes back as `reply {id, value}` / `reply {id, error}`. At most
// 16 calls are in flight per process (the 17th is answered `CALL_LIMIT`); replies may leave
// in any order; calls still open when the process exits are interrupted.
import type { HostMsg, JobError } from "@finisher/core-types";
import { Cause, Effect, Exit, Fiber } from "effect";
import { toFinError } from "../errors.ts";
import type { CallCtx, OnCall } from "./kinds.ts";

export const MAX_CALLS_IN_FLIGHT = 16;

/** Runs an Effect in the engine's services (logger, clock). */
export type RunFork = <A, E>(effect: Effect.Effect<A, E>) => Fiber.Fiber<A, E>;

function replyError(code: string, message: string, details: Record<string, unknown> = {}): JobError {
  return { code, message, details };
}

function errorOf(cause: Cause.Cause<unknown>): JobError {
  const e = toFinError(Cause.squash(cause));
  const details = e.details !== null && typeof e.details === "object" && !Array.isArray(e.details) ? (e.details as Record<string, unknown>) : { value: e.details };
  return { code: e.code, message: e.message, details };
}

/** One process's host calls. */
export class CallRouter {
  readonly #open = new Map<number, Fiber.Fiber<unknown, unknown>>();
  readonly #onCall: OnCall | undefined;
  readonly #ctx: () => CallCtx;
  readonly #send: (msg: HostMsg) => unknown;
  readonly #run: RunFork;
  #closed = false;

  /**
   * `ctx` is read at each call (the job's view changes); `send` writes to the process and
   * reports what it did (a reply nobody can read is dropped).
   */
  constructor(onCall: OnCall | undefined, ctx: () => CallCtx, send: (msg: HostMsg) => unknown, run: RunFork) {
    this.#onCall = onCall;
    this.#ctx = ctx;
    this.#send = send;
    this.#run = run;
  }

  /** How many calls are open. */
  get inFlight(): number {
    return this.#open.size;
  }

  #reply(id: number, outcome: { value: unknown } | { error: JobError }): void {
    if (this.#closed) return;
    const msg: HostMsg = "error" in outcome ? { t: "reply", id, error: outcome.error } : { t: "reply", id, value: outcome.value };
    try {
      this.#send(msg);
    } catch (e) {
      // The value cannot be sent (over the 1 MiB line, or not JSON): answer with an error instead.
      const err = toFinError(e);
      this.#send({ t: "reply", id, error: replyError("INTERNAL", `the reply could not be sent: ${err.message}`, { method: "reply" }) });
    }
  }

  /** A `call` message from the process. */
  call(id: number, method: string, params: unknown): void {
    if (this.#closed) return;
    if (this.#open.has(id)) {
      this.#reply(id, { error: replyError("INVALID_REQUEST", `call ${id} is already open`, { id }) });
      return;
    }
    if (this.#open.size >= MAX_CALLS_IN_FLIGHT) {
      this.#reply(id, { error: replyError("CALL_LIMIT", `at most ${MAX_CALLS_IN_FLIGHT} host calls may be open`, { limit: MAX_CALLS_IN_FLIGHT }) });
      return;
    }
    const onCall = this.#onCall;
    if (onCall === undefined) {
      this.#reply(id, { error: replyError("INVALID_REQUEST", `no host call ${method}`, { method }) });
      return;
    }
    const effect = Effect.suspend(() => onCall(this.#ctx(), method, params));
    const fiber = this.#run(Effect.exit(effect));
    this.#open.set(id, fiber as Fiber.Fiber<unknown, unknown>);
    fiber.addObserver((exit) => {
      this.#open.delete(id);
      if (Exit.isFailure(exit)) return; // interrupted at close (the process is gone)
      const inner = exit.value;
      if (Exit.isSuccess(inner)) this.#reply(id, { value: inner.value });
      else if (!Cause.hasInterruptsOnly(inner.cause)) this.#reply(id, { error: errorOf(inner.cause) });
    });
  }

  /** The process exited: every open call is interrupted and no reply is sent. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    const open = [...this.#open.values()];
    this.#open.clear();
    if (open.length > 0) this.#run(Fiber.interruptAll(open));
  }
}
