// A contract suite any `ProviderAdapter` must pass (plan 7A Task 6a): the fake now, the built-in
// loop with a scripted model client (7A-2), the CLI adapters with their fake CLIs (7D, 7E).
import { Effect, Fiber, Stream } from "effect";
import { describe, expect, it } from "vitest";
import { type BoundEvent, type ProviderAdapter, type SessionSpec, toProviderEvent } from "../../src/agent/adapter.ts";

export interface ContractSubject {
  readonly name: string;
  /** A fresh adapter and a spec whose first turn, prompted with `prompt`, ends by itself. */
  make(): { adapter: ProviderAdapter; spec: SessionSpec; prompt: string };
}

const TERMINAL = new Set(["turnCompleted", "turnFailed", "exited"]);

/** Opens a session, runs one turn and collects its events up to the first terminal one. */
export function oneTurn(adapter: ProviderAdapter, spec: SessionSpec, prompt: string): Promise<{ instance: string; events: BoundEvent[] }> {
  return Effect.runPromise(
    Effect.scoped(
      Effect.gen(function* () {
        const session = yield* adapter.open(spec);
        const collect = yield* Effect.forkChild(
          session.events.pipe(
            Stream.takeUntil((b) => TERMINAL.has(b.event.type)),
            Stream.runCollect,
          ),
        );
        yield* session.startTurn({ runId: "run-1", text: prompt });
        const events = yield* Fiber.join(collect);
        return { instance: session.instance, events: [...events] };
      }),
    ).pipe(Effect.timeout("10 seconds")),
  );
}

export function adapterContractSuite(subject: ContractSubject): void {
  describe(`adapter contract: ${subject.name}`, () => {
    it("detect reports a version and every capability", async () => {
      const { adapter } = subject.make();
      const d = await Effect.runPromise(adapter.detect());
      expect(typeof d.installed).toBe("boolean");
      expect(d.version === null || typeof d.version === "string").toBe(true);
      for (const k of ["steer", "rollback", "resume", "reasoning", "images", "parallelTools", "costReported", "exactCapture", "nativeSubagents"]) {
        expect(typeof (d.capabilities as unknown as Record<string, unknown>)[k], k).toBe("boolean");
      }
    });

    it("a turn's events decode, carry the instance, start with turnStarted and end once", async () => {
      const { adapter, spec, prompt } = subject.make();
      const { instance, events } = await oneTurn(adapter, spec, prompt);
      expect(events.length).toBeGreaterThanOrEqual(2);
      expect(events[0]!.event.type).toBe("turnStarted");
      expect(TERMINAL.has(events.at(-1)!.event.type)).toBe(true);
      expect(events.filter((b) => TERMINAL.has(b.event.type))).toHaveLength(1);
      for (const b of events) {
        expect(b.instance).toBe(instance);
        expect(toProviderEvent(JSON.parse(JSON.stringify(b.event)))).toEqual(b.event);
      }
    });

    it("every toolCompleted follows its own toolStarted, once", async () => {
      const { adapter, spec, prompt } = subject.make();
      const { events } = await oneTurn(adapter, spec, prompt);
      const started = new Set<string>();
      const done = new Set<string>();
      for (const { event: e } of events) {
        if (e.type === "toolStarted") {
          expect(started.has(e.providerId)).toBe(false);
          started.add(e.providerId);
        }
        if (e.type === "toolCompleted") {
          expect(started.has(e.providerId)).toBe(true);
          expect(done.has(e.providerId)).toBe(false);
          done.add(e.providerId);
        }
      }
    });

    it("interrupt without a turn does nothing; closing the scope ends the stream; ref is null or {kind, id}", async () => {
      const { adapter, spec } = subject.make();
      const out = await Effect.runPromise(
        Effect.gen(function* () {
          const { session, events } = yield* Effect.scoped(
            Effect.gen(function* () {
              const session = yield* adapter.open(spec);
              yield* session.interrupt();
              const events = yield* Effect.forkDetach(Stream.runCollect(session.events));
              return { session, events };
            }),
          );
          const left = yield* Fiber.join(events);
          return { ref: session.ref(), left: [...left] };
        }).pipe(Effect.timeout("10 seconds")),
      );
      expect(out.left.filter((b) => b.event.type !== "unknown")).toEqual([]);
      if (out.ref !== null) {
        expect(typeof out.ref.kind).toBe("string");
        expect(typeof out.ref.id).toBe("string");
      }
    });
  });
}
