// A turn's tail (plan 7D Task 9 review; Review Focus 4): an instance that is closed or replaced
// still owes the usage it already queued. `closeLive` drains it deterministically, and the
// runtime charges it to the run that instance served, then enforces the crossing against the
// session's live state (a replacement run stops, a `budget` approval opens).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { type Cause, Effect, Queue, Scope, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type BoundEvent, NO_CAPABILITIES, type ProviderAdapter, type ProviderSession, type SessionSpec } from "../src/agent/adapter.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { closeLive, openLive } from "../src/agent/runtime-live.ts";
import { AgentStore } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

const USAGE = (input: number): ProviderEvent => ({ type: "usage", input, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 0 });
const SPEC = { sessionId: "s1", projectId: "p1", model: "m", role: "editor", mode: "draft", system: "", cwd: "/w", env: {}, providerRef: null } as unknown as SessionSpec;
const ENV = { system: "", cwd: "/w", env: {} };

/** A plain adapter: `n` events queued before the close, usage last; its scope ends the stream unless `endless`. */
function backlog(n: number, endless = false): ProviderAdapter {
  return {
    harness: "fake",
    detect: () => Effect.die("unused"),
    open: () =>
      Effect.gen(function* () {
        const queue = yield* Queue.unbounded<BoundEvent, Cause.Done>();
        if (!endless) yield* Effect.addFinalizer(() => Queue.end(queue));
        const session: ProviderSession = {
          instance: "i1",
          events: Stream.fromQueue(queue),
          startTurn: () => Effect.void,
          steer: () => Effect.void,
          interrupt: () => Effect.void,
          respond: () => Effect.void,
          rollback: () => Effect.void,
          ref: () => null,
        };
        for (let i = 0; i < n - 1; i++) Queue.offerUnsafe(queue, { instance: "i1", event: { type: "messageDelta", text: "x" } });
        Queue.offerUnsafe(queue, { instance: "i1", event: USAGE(7) });
        return session;
      }) as Effect.Effect<ProviderSession, FinError, Scope.Scope>,
  };
}

describe("closeLive drains the instance's queue", () => {
  for (const n of [10_000, 100_000]) {
    test(`a backlog of ${n} with usage last is forwarded whole`, async () => {
      const seen: ProviderEvent[] = [];
      const live = await openLive(backlog(n), SPEC, ENV, NO_CAPABILITIES, (b) => void seen.push(b.event), () => {});
      await closeLive(live);
      expect(seen).toHaveLength(n);
      expect(seen.at(-1)).toEqual(USAGE(7));
    }, 30_000);
  }

  test("a stream that never ends is cut after the bound, and logged", async () => {
    const logs: string[] = [];
    const live = await openLive(backlog(3, true), SPEC, ENV, NO_CAPABILITIES, () => {}, () => {});
    const t0 = Date.now();
    await closeLive(live, (m) => void logs.push(m), 200);
    expect(Date.now() - t0).toBeLessThan(5_000);
    expect(logs).toEqual([expect.stringMatching(/drain/)]);
  });
});

let dir: string;
let control: ControlStore;
let store: AgentStore;
const cleanups: (() => Promise<void>)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-usage-tail-"));
  const clock = manualClock(Date.now());
  control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  store = new AgentStore(control, dir, clock, systemIds(clock));
});

afterEach(async () => {
  for (const c of cleanups.splice(0)) await c();
  await control.close();
  rmSync(dir, { recursive: true, force: true });
});

async function until(f: () => boolean, what: string, ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

/** Run 1 crashes owing 500 tokens; run 2 (the replacement) waits for an answer that never comes. */
const CRASHED: FakeScript = { turns: [[{ late: [USAGE(500)] }, { crash: { code: null, signal: "SIGKILL" } }]] };
const WAITING: FakeScript = { turns: [[{ awaitAnswer: "never" }]] };

async function replaced(budget?: { tokens: number }) {
  const h = makeRuntime({ store, control }, FakeAdapter([CRASHED, WAITING]), { clock: manualClock(Date.now()), tools: {} });
  cleanups.push(() => h.runtime.close());
  await h.runtime.boot();
  const { session } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "go", ...(budget === undefined ? {} : { budget }) }, PEOPLE, META));
  await until(() => store.get(session.id)!.status === "interrupted", "the crash");
  const run1 = store.runs(session.id).at(-1)!.id;
  await Effect.runPromise(h.runtime.resume(session.id, PEOPLE, "again"));
  const run2 = store.runs(session.id).at(-1)!.id;
  expect(run2).not.toBe(run1);
  return { h, id: session.id, run1, run2 };
}

describe("a replaced instance's tail", () => {
  test("is charged to the run its instance served, not the replacement's", async () => {
    const { id, run1, run2 } = await replaced();
    await until(() => store.get(id)!.usage.input === 500, "the tail charged");
    expect(store.run(run1)!.usage.input).toBe(500);
    expect(store.run(run2)!.usage.input).toBe(0);
    expect(store.nodes(id).filter((n) => n.kind === "usage").map((n) => n.runId)).toEqual([run1]);
    expect(store.get(id)!.status).toBe("running");
  });

  test("past the limit stops the replacement run with BUDGET_EXCEEDED and a budget approval", async () => {
    const { id, run1 } = await replaced({ tokens: 300 });
    await until(() => store.get(id)!.status === "interrupted" && store.approvals(id).length > 0, "the budget stop");
    expect(store.get(id)).toMatchObject({ status: "interrupted", statusReason: "BUDGET_EXCEEDED" });
    expect(store.run(run1)!.usage.input).toBe(500);
    expect(store.approvals(id)).toMatchObject([{ kind: "budget", status: "open", subject: { kind: "tokens", limit: 300 } }]);
  });
});
