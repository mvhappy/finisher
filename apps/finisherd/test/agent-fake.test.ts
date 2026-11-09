// The adapter contract and the scripted fake provider (plan 7A Task 6a, Decision 5).
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Deferred, Effect, Fiber, Stream } from "effect";
import { describe, expect, it } from "vitest";
import { type BoundEvent, type ProviderSession, type SessionSpec, toProviderEvent } from "../src/agent/adapter.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { fakeScriptsOf } from "../src/agent/wiring.ts";
import { adapterContractSuite, oneTurn } from "./support/adapter-contract.ts";

const script = (name: string): FakeScript =>
  JSON.parse(readFileSync(new URL(`./fixtures/fake-scripts/${name}.json`, import.meta.url), "utf8")) as FakeScript;

const spec = (sessionId = "01JC0000000000000000000003"): SessionSpec => ({
  sessionId,
  projectId: "01jb0000000000000000000001",
  model: "test/closed-explicit",
  role: "editor",
  mode: "draft",
  system: "You edit video with fin.",
  cwd: "/nonexistent/workspace",
  env: {},
  providerRef: null,
});

const types = (events: BoundEvent[]) => events.map((b) => b.event.type);
const TERMINAL = new Set(["turnCompleted", "turnFailed", "exited"]);

/** Collects events of `session` until a terminal one, forked before `act` runs. */
const until = (session: ProviderSession, act: Effect.Effect<void, unknown>) =>
  Effect.gen(function* () {
    const f = yield* Effect.forkChild(session.events.pipe(Stream.takeUntil((b) => TERMINAL.has(b.event.type)), Stream.runCollect));
    yield* act;
    return [...(yield* Fiber.join(f))];
  });

const run = <A>(e: Effect.Effect<A, unknown, never>) => Effect.runPromise(e.pipe(Effect.timeout("10 seconds")));

adapterContractSuite({ name: "fake", make: () => ({ adapter: FakeAdapter([script("simple")]), spec: spec(), prompt: "tighten the intro" }) });

describe("fake provider", () => {
  it("emits the scripted events in order, per session and per turn", async () => {
    const adapter = FakeAdapter([script("simple")]);
    const out = await run(
      Effect.scoped(
        Effect.gen(function* () {
          const s = yield* adapter.open(spec());
          const first = yield* until(s, s.startTurn({ runId: "r1", text: "one", facts: "budget: 10 USD" }));
          const second = yield* until(s, s.startTurn({ runId: "r2", text: "two" }));
          return { first, second, instance: s.instance };
        }),
      ),
    );
    expect(types(out.first)).toEqual(["turnStarted", "messageDelta", "reasoning", "message", "usage", "turnCompleted"]);
    expect(types(out.second)).toEqual(["turnStarted", "message", "turnCompleted"]);
    expect(new Set([...out.first, ...out.second].map((b) => b.instance))).toEqual(new Set([out.instance]));
    expect(adapter.log.turns).toEqual([
      { instance: out.instance, runId: "r1", text: "one", facts: "budget: 10 USD" },
      { instance: out.instance, runId: "r2", text: "two" },
    ]);
  });

  it("waits for each tool result and emits its toolCompleted", async () => {
    const adapter = FakeAdapter([script("tool")]);
    const events = await run(
      Effect.scoped(
        Effect.gen(function* () {
          const s = yield* adapter.open(spec());
          const f = yield* Effect.forkChild(until(s, s.startTurn({ runId: "r1", text: "trim" })));
          yield* s.respond("call_2", { kind: "toolResult", ok: false, output: { code: "CONFLICT" } }); // early: kept
          yield* s.respond("call_1", { kind: "toolResult", ok: true, output: { view: "…" } });
          return yield* Fiber.join(f);
        }),
      ),
    );
    expect(types(events)).toEqual(["turnStarted", "toolStarted", "toolCompleted", "toolStarted", "toolCompleted", "message", "usage", "turnCompleted"]);
    expect(events[4]!.event).toEqual({ type: "toolCompleted", providerId: "call_2", ok: false, output: { code: "CONFLICT" } });
  });

  it("a scripted spawn failure is PROVIDER_EXITED", async () => {
    const adapter = FakeAdapter([script("spawn-fail")]);
    const exit = await Effect.runPromiseExit(Effect.scoped(adapter.open(spec())));
    expect(exit._tag).toBe("Failure");
    const err = await Effect.runPromise(Effect.scoped(adapter.open(spec())).pipe(Effect.flip));
    expect(err.code).toBe("PROVIDER_EXITED");
  });

  it("a crash emits exited; the next turn is PROVIDER_EXITED; late events keep the old binding", async () => {
    const adapter = FakeAdapter([script("crash-late"), script("simple")]);
    const out = await run(
      Effect.scoped(
        Effect.gen(function* () {
          const old = yield* adapter.open(spec());
          const crashed = yield* until(old, old.startTurn({ runId: "r1", text: "go" }));
          const refused = yield* old.startTurn({ runId: "r2", text: "again" }).pipe(Effect.flip);
          // The old instance's stream stays open for what it still owes.
          const lateFiber = yield* Effect.forkChild(Stream.runCollect(old.events));
          const replacement = yield* adapter.open(spec());
          const late = [...(yield* Fiber.join(lateFiber))];
          const fresh = yield* until(replacement, replacement.startTurn({ runId: "r2", text: "again" }));
          return { crashed, refused, late, fresh, old: old.instance, now: replacement.instance };
        }),
      ),
    );
    expect(types(out.crashed)).toEqual(["turnStarted", "toolStarted", "exited"]);
    expect(out.crashed.at(-1)!.event).toEqual({ type: "exited", code: null, signal: "SIGKILL" });
    expect(out.refused.code).toBe("PROVIDER_EXITED");
    expect(out.late.map((b) => [b.instance, b.event.type])).toEqual([
      [out.old, "toolCompleted"],
      [out.old, "messageDelta"],
    ]);
    expect(out.now).not.toBe(out.old);
    expect(out.fresh.every((b) => b.instance === out.now)).toBe(true);
  });

  it("an unknown event passes through as a notice-able event, never a failure", async () => {
    const { events } = await oneTurn(FakeAdapter([script("unknown")]), spec(), "hi");
    expect(events.map((b) => b.event)).toEqual([
      { type: "turnStarted" },
      { type: "unknown", name: "rateLimitUpdate" },
      { type: "message", role: "assistant", blocks: [{ type: "text", text: "Still here." }] },
      { type: "turnCompleted", stopReason: "endTurn" },
    ]);
  });

  it("a known event that does not decode fails the turn with PROVIDER_PROTOCOL", async () => {
    const { events } = await oneTurn(FakeAdapter([script("protocol")]), spec(), "hi");
    expect(types(events)).toEqual(["turnStarted", "turnFailed"]);
    const last = events.at(-1)!.event;
    expect(last.type === "turnFailed" && last.error.code).toBe("PROVIDER_PROTOCOL");
  });

  it("interrupt ends a waiting turn; a second turn while one runs is SESSION_BUSY; respond and rollback", async () => {
    const adapter = FakeAdapter([script("approval")]);
    const out = await run(
      Effect.scoped(
        Effect.gen(function* () {
          const s = yield* adapter.open(spec());
          const asked = yield* Deferred.make<void>();
          const f = yield* Effect.forkChild(
            s.events.pipe(
              Stream.tap((b) => (b.event.type === "request" ? Deferred.succeed(asked, undefined) : Effect.void)),
              Stream.takeUntil((b) => TERMINAL.has(b.event.type)),
              Stream.runCollect,
            ),
          );
          yield* s.startTurn({ runId: "r1", text: "merge it" });
          yield* Deferred.await(asked);
          const busy = yield* s.startTurn({ runId: "r2", text: "x" }).pipe(Effect.flip);
          yield* s.steer({ text: "only the intro" });
          yield* s.interrupt();
          const events = [...(yield* Fiber.join(f))];
          yield* s.rollback(0);
          return { busy, events, ref: s.ref() };
        }),
      ),
    );
    expect(out.busy.code).toBe("SESSION_BUSY");
    expect(types(out.events)).toEqual(["turnStarted", "request", "turnCompleted"]);
    expect(out.events.at(-1)!.event).toEqual({ type: "turnCompleted", stopReason: "interrupted" });
    expect(adapter.log.steers.map((x) => x.text)).toEqual(["only the intro"]);
    expect(adapter.log.rollbacks).toHaveLength(1);
    expect(out.ref).toEqual({ kind: "fake", id: adapter.log.opened[0]!.instance });
    const noRollback = FakeAdapter([script("simple")]);
    const err = await Effect.runPromise(Effect.scoped(Effect.flatMap(noRollback.open(spec()), (s) => s.rollback(0))).pipe(Effect.flip));
    expect(err.code).toBe("PROVIDER_UNAVAILABLE");
  });
});

describe("toProviderEvent", () => {
  it("maps an untyped message to PROVIDER_PROTOCOL and an unknown type to unknown", () => {
    expect(toProviderEvent({})).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    expect(toProviderEvent(null)).toMatchObject({ code: "PROVIDER_PROTOCOL" });
    expect(toProviderEvent({ type: "x".repeat(200) })).toEqual({ type: "unknown", name: "x".repeat(64) });
    expect(toProviderEvent({ type: "unknown", name: "spoof" })).toEqual({ type: "unknown", name: "unknown" });
    expect(toProviderEvent({ type: "usage", input: 1, output: 1, cacheRead: 0, cacheWrite: 0, reasoning: 0 })).toEqual({
      type: "usage",
      input: 1,
      output: 1,
      cacheRead: 0,
      cacheWrite: 0,
      reasoning: 0,
    });
  });
});

describe("the fake's scripts from a file (FIN_TEST_FAKE_SCRIPTS, the e2e `sandbox` scenario)", () => {
  const dir = mkdtempSync(join(tmpdir(), "fake-scripts-"));
  const file = (name: string, text: string) => {
    const p = join(dir, name);
    writeFileSync(p, text);
    return p;
  };
  const one: FakeScript = { name: "children", turns: [[{ spawn: ["/bin/true"] }]] };

  it("reads the scripts the variable names", () => {
    expect(fakeScriptsOf(undefined, { FIN_TEST_FAKE_SCRIPTS: file("a.json", JSON.stringify([one])) })).toEqual([one]);
  });

  it("in-process options win, and without either the default script runs", () => {
    const other: FakeScript = { name: "other", turns: [[]] };
    expect(fakeScriptsOf([other], { FIN_TEST_FAKE_SCRIPTS: file("b.json", JSON.stringify([one])) })).toEqual([other]);
    expect(fakeScriptsOf(undefined, {})[0]?.name).toBe("echo");
  });

  it("refuses a file that is not a list of scripts", () => {
    for (const [name, text] of [["c.json", "{}"], ["d.json", "[]"], ["e.json", "[{\"turns\": 1}]"], ["f.json", "not json"]] as const) {
      expect(() => fakeScriptsOf(undefined, { FIN_TEST_FAKE_SCRIPTS: file(name, text) }), name).toThrow(/FIN_TEST_FAKE_SCRIPTS/);
    }
  });
});
