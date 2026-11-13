// One open provider instance of a session (plan 7A Task 6b): its scope, the pump that forwards
// its bound events to the runtime, and the per-run bookkeeping the engine keeps beside the
// store (calls in flight, a stop waiting for a safe point, held steers, budget reservations).
import type { FinError } from "@finisher/contracts";
import { Cause, Effect, Exit, Fiber, Option, Scope, Stream } from "effect";
import type { BoundEvent, Capabilities, ProviderAdapter, ProviderSession, SessionSpec } from "./adapter.ts";
import type { Reading, Reservation } from "./budget.ts";
import type { OpenEnvironment } from "./runtime-deps.ts";
import type { SessionSandbox } from "./sandbox.ts";

export interface Call {
  readonly callId: string;
  readonly runId: string;
}

export interface Live {
  readonly sessionId: string;
  readonly provider: ProviderSession;
  readonly instance: string;
  readonly caps: Capabilities;
  readonly env: OpenEnvironment;
  /** A CLI harness's session sandbox, opened in `scope` (plan 7C Task 11). */
  readonly sandbox: SessionSandbox | undefined;
  /** The sandbox's notices are written (once per instance). */
  noticed?: boolean;
  readonly scope: Scope.Closeable;
  pump: Fiber.Fiber<void, never> | null;
  /** Set once its scope is being closed. */
  closed: boolean;
  /** The run this instance is working on, else null. */
  runId: string | null;
  /** Announced calls without a result, by the provider's id. */
  readonly calls: Map<string, Call>;
  /** Calls the runtime is executing, by call id. */
  readonly running: Map<string, AbortController>;
  /** A budget stop waiting for the running tool calls to finish (the safe point). */
  pendingStop: Reading | null;
  /** When a pending stop stops waiting and aborts the running calls (`stopGraceMs`). */
  stopDeadline: ReturnType<typeof setTimeout> | null;
  /** Steers for a provider that cannot take them mid-run: the next run's prompt. */
  readonly held: string[];
  /** Tokens and money held by `CallGate.beforeCall`, settled by `usage` events, oldest first. */
  readonly reservations: (Reservation | null)[];
}

/**
 * Opens `adapter` for `spec` in a scope of its own and starts pumping its events to `onEvent`.
 * `onEnd` runs when the event stream ends by itself (the instance is gone). `sandbox` (a CLI
 * harness's) is opened first in the same scope, so the instance's end kills its processes;
 * the adapter gets it as `spec.sandbox` and its environment as `spec.env`.
 */
export async function openLive(
  adapter: ProviderAdapter,
  spec: SessionSpec,
  env: OpenEnvironment,
  caps: Capabilities,
  onEvent: (b: BoundEvent) => void,
  onEnd: (live: Live) => void,
  sandbox?: Effect.Effect<SessionSandbox, FinError, Scope.Scope>,
): Promise<Live> {
  const scope = await Effect.runPromise(Scope.make());
  const exit = await Effect.runPromiseExit(
    Scope.provide(scope)(
      Effect.gen(function* () {
        const box = sandbox === undefined ? undefined : yield* sandbox;
        const provider = yield* adapter.open(box === undefined ? spec : { ...spec, env: box.env, sandbox: box });
        return { provider, box };
      }),
    ),
  );
  if (Exit.isFailure(exit)) {
    await Effect.runPromise(Scope.close(scope, Exit.void));
    throw failureOf(exit);
  }
  const { provider, box } = exit.value;
  const live: Live = {
    sessionId: spec.sessionId,
    provider,
    instance: provider.instance,
    caps,
    env: box === undefined ? env : { ...env, env: box.env },
    sandbox: box,
    scope,
    pump: null,
    closed: false,
    runId: null,
    calls: new Map(),
    running: new Map(),
    pendingStop: null,
    stopDeadline: null,
    held: [],
    reservations: [],
  };
  live.pump = Effect.runFork(
    Stream.runForEach(provider.events, (b) => Effect.sync(() => onEvent(b))).pipe(
      Effect.catchCause(() => Effect.void),
      Effect.ensuring(Effect.sync(() => onEnd(live))),
    ),
  );
  return live;
}

/** Stops the pump and closes the instance's scope (the adapter's finalizers end it). */
export async function closeLive(live: Live): Promise<void> {
  live.closed = true;
  if (live.stopDeadline !== null) clearTimeout(live.stopDeadline);
  live.stopDeadline = null;
  const pump = live.pump;
  live.pump = null;
  // What the instance already queued is pumped first (7D Task 9: an interrupt's `result` and its
  // usage, charged as the turn's tail), then the pump stops.
  for (let tick = 0; pump && tick < 2; tick++) await new Promise((r) => setImmediate(r));
  if (pump) await Effect.runPromise(Fiber.interrupt(pump));
  await Effect.runPromise(Scope.close(live.scope, Exit.void));
}

/** Closes the scope once the pump has ended by itself (a detached, exited instance). */
export function closeScope(live: Live): Promise<void> {
  live.closed = true;
  return Effect.runPromise(Scope.close(live.scope, Exit.void));
}

/** Runs a provider call, answering its error (if any) rather than throwing. */
export async function call(e: Effect.Effect<void, FinError>): Promise<FinError | null> {
  const exit = await Effect.runPromiseExit(e);
  return Exit.isFailure(exit) ? failureOf(exit) : null;
}

function failureOf<A>(exit: Exit.Failure<A, FinError>): FinError {
  const err = Cause.findErrorOption(exit.cause);
  if (Option.isSome(err)) return err.value;
  throw Cause.squash(exit.cause);
}
