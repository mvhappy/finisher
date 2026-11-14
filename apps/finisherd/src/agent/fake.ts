// The scripted `fake` harness (sub-project 7 §16.1; plan 7A Task 6a): tests the runtime without a
// model. A script is a list of turns, each a list of steps: emit events (raw, through
// `toProviderEvent`, so an unknown type becomes a notice-able event), wait for a tool result
// or an answer, crash, and leave late events that the instance emits after it is replaced.
// `holdSteers` holds every steer and hands the held back as Claude Code does (`notice
// {steerReturned: {harness, steers: [{text, nodeId?, at?}]}}`): at a `returnHeld` step, in
// the run, and at an interrupt, as the turn's tail.
// Registered only in test builds (7A-2, Task 12).
import { FinError } from "@finisher/contracts";
import { type Cause, Deferred, Effect, Fiber, Queue, Scope, Stream } from "effect";
import { finError } from "../errors.ts";
import {
  type BoundEvent,
  type Capabilities,
  NO_CAPABILITIES,
  type ProviderAdapter,
  type ProviderEvent,
  type ProviderSession,
  type RuntimeAnswer,
  type SessionSpec,
  toProviderEvent,
} from "./adapter.ts";

export type FakeStep =
  /** Emit these events, in order. */
  | { readonly emit: readonly unknown[] }
  /** Wait for `respond(id, {kind: "toolResult"})`, then emit its `toolCompleted`. */
  | { readonly awaitTool: string }
  /** Wait for `respond(id, …)` (an approval or user input). */
  | { readonly awaitAnswer: string }
  /** The process exits: emit `exited` and end the instance. */
  | { readonly crash: { readonly code: number | null; readonly signal: string | null } }
  /** Events this instance emits after the session is opened again (replaced). */
  | { readonly late: readonly unknown[] }
  /**
   * Starts `argv` in the workspace through the session sandbox (a scripted stand-in for a CLI
   * agent's process, plan 7C Task 11) and emits a `notice` named `child` with its exit and
   * output (each at most 4 KiB): what the process could and could not do under the profile.
   */
  | { readonly spawn: readonly string[] }
  /** After `ms`, the held steers handed back (`holdSteers`), in the run. */
  | { readonly returnHeld: number };

export interface FakeScript {
  readonly name?: string;
  /** `open` fails with `PROVIDER_EXITED` (the process could not start). */
  readonly spawnFails?: boolean;
  readonly capabilities?: Partial<Capabilities>;
  /** Steers are held, never taken, and handed back (see the module docs). */
  readonly holdSteers?: boolean;
  readonly turns: readonly (readonly FakeStep[])[];
}

/** What the fake saw, for tests. */
export interface FakeLog {
  readonly opened: { instance: string; spec: SessionSpec; script: string }[];
  readonly turns: { instance: string; runId: string; text: string; facts?: string }[];
  readonly steers: { instance: string; text: string }[];
  readonly answers: { instance: string; id: string; answer: RuntimeAnswer }[];
  readonly rollbacks: { instance: string; toTurn: number }[];
}

type EventQueue = Queue.Queue<BoundEvent, Cause.Done>;

interface Old {
  readonly instance: string;
  readonly queue: EventQueue;
  readonly late: unknown[];
}

const TERMINAL = new Set(["turnCompleted", "turnFailed", "exited"]);

export function FakeAdapter(scripts: readonly FakeScript[]): ProviderAdapter & { readonly log: FakeLog } {
  if (scripts.length === 0) throw new Error("FakeAdapter needs a script");
  const log: FakeLog = { opened: [], turns: [], steers: [], answers: [], rollbacks: [] };
  let opens = 0;
  /** Per session: the instance a reopen replaces, with the late events it still owes. */
  const previous = new Map<string, Old>();
  const caps = (s: FakeScript): Capabilities => ({ ...NO_CAPABILITIES, steer: true, resume: true, ...s.capabilities });

  const open = (spec: SessionSpec) =>
    Effect.gen(function* () {
      const script = scripts[Math.min(opens, scripts.length - 1)]!;
      const n = ++opens;
      const instance = `fake-${n}`;
      if (script.spawnFails) {
        return yield* Effect.fail(finError("PROVIDER_EXITED", "the fake provider could not start", { code: 127 }));
      }
      const queue: EventQueue = yield* Queue.unbounded<BoundEvent, Cause.Done>();
      const scope = yield* Effect.scope;
      log.opened.push({ instance, spec, script: script.name ?? `script-${n}` });

      // The replaced instance emits what it still owed, bound to itself, then ends.
      const old = previous.get(spec.sessionId);
      if (old) {
        for (const raw of old.late) yield* Queue.offer(old.queue, { instance: old.instance, event: asEvent(raw) });
        yield* Queue.end(old.queue);
      }
      const late: unknown[] = [];
      previous.set(spec.sessionId, { instance, queue, late });

      let turn = 0;
      let exited = false;
      let running: Fiber.Fiber<void> | null = null;
      const waits = new Map<string, Deferred.Deferred<RuntimeAnswer>>();
      let held: { text: string; nodeId?: string; at?: number }[] = [];
      const returnHeld = () => {
        const steers = held;
        held = [];
        return steers.length === 0 ? Effect.void : emit({ type: "notice", name: "steerReturned", details: { harness: "fake", steers } });
      };
      const early = new Map<string, RuntimeAnswer>();

      const emit = (event: ProviderEvent) => Queue.offer(queue, { instance, event }).pipe(Effect.asVoid);
      const waitFor = (id: string) =>
        Effect.gen(function* () {
          const ready = early.get(id);
          if (ready) {
            early.delete(id);
            return ready;
          }
          const d = yield* Deferred.make<RuntimeAnswer>();
          waits.set(id, d);
          return yield* Deferred.await(d);
        });

      /** Runs one turn's steps; returns whether the turn reached a terminal event itself. */
      const runSteps = (steps: readonly FakeStep[]) =>
        Effect.gen(function* () {
          yield* emit({ type: "turnStarted" });
          for (const step of steps) {
            if ("emit" in step) {
              for (const raw of step.emit) {
                const e = toProviderEvent(raw);
                if (e instanceof FinError) {
                  yield* emit({ type: "turnFailed", error: { code: e.code, message: e.message, details: e.details } });
                  return;
                }
                yield* emit(e);
                if (TERMINAL.has(e.type)) return;
              }
            } else if ("awaitTool" in step) {
              const a = yield* waitFor(step.awaitTool);
              if (a.kind !== "toolResult") {
                yield* emit({ type: "turnFailed", error: { code: "PROVIDER_PROTOCOL", message: "expected a tool result", details: { id: step.awaitTool } } });
                return;
              }
              yield* emit({ type: "toolCompleted", providerId: step.awaitTool, ok: a.ok, output: a.output });
            } else if ("awaitAnswer" in step) {
              yield* waitFor(step.awaitAnswer);
            } else if ("spawn" in step) {
              yield* emit({ type: "notice", name: "child", details: yield* runChild(spec, step.spawn) });
            } else if ("returnHeld" in step) {
              yield* Effect.sleep(step.returnHeld);
              yield* returnHeld();
            } else if ("late" in step) {
              late.push(...step.late);
            } else if ("crash" in step) {
              exited = true;
              yield* emit({ type: "exited", code: step.crash.code, signal: step.crash.signal });
              if (late.length === 0) yield* Queue.end(queue);
              return;
            }
          }
          yield* emit({ type: "turnCompleted", stopReason: "endTurn" });
        });

      yield* Scope.addFinalizer(
        scope,
        Effect.gen(function* () {
          if (running) yield* Fiber.interrupt(running);
          if (previous.get(spec.sessionId)?.instance === instance && late.length === 0) previous.delete(spec.sessionId);
          yield* Queue.end(queue);
        }),
      );

      const exitedError = () => finError("PROVIDER_EXITED", "the fake provider has exited", { instance }, "resume the session");

      const session: ProviderSession = {
        instance,
        events: Stream.fromQueue(queue),
        startTurn: (i) =>
          Effect.gen(function* () {
            if (exited) return yield* Effect.fail(exitedError());
            if (running && running.pollUnsafe() === undefined) {
              return yield* Effect.fail(finError("SESSION_BUSY", "a turn is running", { instance }));
            }
            log.turns.push({ instance, runId: i.runId, text: i.text, ...(i.facts === undefined ? {} : { facts: i.facts }) });
            const steps = script.turns[turn++] ?? [];
            running = yield* Effect.forkIn(runSteps(steps), scope);
          }),
        steer: (i) =>
          Effect.gen(function* () {
            if (exited) return yield* Effect.fail(exitedError());
            log.steers.push({ instance, text: i.text });
            if (script.holdSteers) held.push({ text: i.text, ...(i.nodeId === undefined ? {} : { nodeId: i.nodeId }), ...(i.at === undefined ? {} : { at: i.at }) });
          }),
        interrupt: () =>
          Effect.gen(function* () {
            const fiber = running;
            if (!exited) yield* returnHeld();
            if (fiber === null || fiber.pollUnsafe() !== undefined) return;
            yield* Fiber.interrupt(fiber);
            if (!exited) yield* emit({ type: "turnCompleted", stopReason: "interrupted" });
          }),
        respond: (id, answer) =>
          Effect.gen(function* () {
            if (exited) return yield* Effect.fail(exitedError());
            log.answers.push({ instance, id, answer });
            const d = waits.get(id);
            if (d) {
              waits.delete(id);
              yield* Deferred.succeed(d, answer);
            } else {
              early.set(id, answer);
            }
          }),
        rollback: (toTurn) =>
          Effect.gen(function* () {
            if (!caps(script).rollback) {
              return yield* Effect.fail(finError("PROVIDER_UNAVAILABLE", "the fake provider cannot roll back", { instance }));
            }
            log.rollbacks.push({ instance, toTurn });
            turn = Math.min(turn, toTurn);
          }),
        ref: () => ({ kind: "fake", id: instance }),
      };
      return session;
    });

  return {
    harness: "fake",
    log,
    detect: () =>
      Effect.succeed({
        installed: true,
        version: "fake-1",
        supported: "fake-1",
        signedIn: true,
        capabilities: caps(scripts[0]!),
      }),
    open: (spec) => open(spec) as Effect.Effect<ProviderSession, FinError, Scope.Scope>,
  };
}

/** Runs one scripted child in `spec`'s sandbox and answers its exit and output. */
function runChild(spec: SessionSpec, argv: readonly string[]): Effect.Effect<Record<string, unknown>> {
  const box = spec.sandbox;
  if (box === undefined) return Effect.succeed({ error: "NO_SANDBOX" });
  return box.spawn(argv, { cwd: spec.cwd, stdio: "pipe" }).pipe(
    Effect.flatMap((child) =>
      Effect.promise(async () => {
        child.stdin.end();
        const read = async (r: AsyncIterable<Buffer | string>) => {
          let out = "";
          for await (const c of r) out += String(c);
          return out.slice(0, 4096);
        };
        const [stdout, stderr, exit] = await Promise.all([read(child.stdout), read(child.stderr), child.exited]);
        return { code: exit.code, signal: exit.signal, stdout, stderr };
      }),
    ),
    Effect.catch((e) => Effect.succeed({ error: e.code, message: e.message })),
  );
}

/** A late event that does not decode is still bound to its instance, as a notice-able event. */
function asEvent(raw: unknown): ProviderEvent {
  const e = toProviderEvent(raw);
  return e instanceof FinError ? { type: "unknown", name: "malformed" } : e;
}
