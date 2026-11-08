// Host calls (5C-1 plan Decision 5a): routing to `onCall` with a `CallCtx`, error replies, the
// 16-call limit and interruption at exit.
import type { HostMsg } from "@finisher/core-types";
import { FinError } from "@finisher/contracts";
import { Duration, Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { CallRouter, MAX_CALLS_IN_FLIGHT } from "../../src/jobs/calls.ts";
import type { CallCtx, OnCall } from "../../src/jobs/kinds.ts";
import { closeTestEngines, OWNER, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

const ctx = (): CallCtx => ({ job: { id: "j1" } as never, role: "worker", owner: OWNER });

function router(onCall: OnCall | undefined) {
  const sent: HostMsg[] = [];
  const r = new CallRouter(onCall, ctx, (m) => (sent.push(m), "ok"), (e) => Effect.runFork(e));
  return { r, sent };
}

describe("host calls through a worker", () => {
  test("call_reaches_on_call_with_ctx", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.call", { method: "test.echo", params: [1, 2] });
    expect(v.state).toBe("succeeded");
    expect(v.result).toEqual({ echo: [1, 2], job: v.id, role: "worker", owner: OWNER });
  });

  test("call_error_is_replied", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.call", { method: "test.fail", params: { a: 1 } });
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "TEST_FAIL", details: { a: 1 } });
  });

  test("unknown_method_is_invalid_request", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.call", { method: "test.nothing" });
    expect(v.error).toMatchObject({ code: "INVALID_REQUEST", details: { method: "test.nothing" } });
    const none = await testEngine({ testKinds: { families: { test: { onCall: undefined } as never } } });
    const w = await none.runJob("test.call", { method: "test.echo" });
    expect(w.error).toMatchObject({ code: "INVALID_REQUEST", details: { method: "test.echo" } });
  });
});

describe("the call router", () => {
  test("seventeenth_call_is_call_limit", async () => {
    const { r, sent } = router((_c, _m, p) => Effect.sleep(Duration.millis(100)).pipe(Effect.as(p)));
    for (let id = 1; id <= MAX_CALLS_IN_FLIGHT + 1; id++) r.call(id, "test.slow", id);
    expect(r.inFlight).toBe(16);
    expect(sent).toEqual([{ t: "reply", id: 17, error: { code: "CALL_LIMIT", message: expect.any(String), details: { limit: 16 } } }]);
    await until(() => sent.length === 17);
    // Replies come as the calls end; once one ended, a new call is taken.
    expect(sent.slice(1).map((m) => (m as { value: unknown }).value).sort((a, b) => Number(a) - Number(b))).toEqual(
      Array.from({ length: 16 }, (_, i) => i + 1),
    );
    r.call(18, "test.slow", 18);
    expect(r.inFlight).toBe(1);
  });

  test("open_calls_interrupted_at_exit", async () => {
    let interrupted = 0;
    const { r, sent } = router(() => Effect.never.pipe(Effect.onInterrupt(() => Effect.sync(() => interrupted++))));
    r.call(1, "a", null);
    r.call(2, "b", null);
    await new Promise((res) => setTimeout(res, 10));
    r.close();
    await until(() => interrupted === 2);
    expect(sent).toEqual([]);
    r.call(3, "c", null);
    expect(sent).toEqual([]);
  });

  test("errors_and_defects_become_reply_errors", async () => {
    const { r, sent } = router((_c, m) =>
      m === "fail" ? Effect.fail(new FinError({ code: "NOPE", message: "no", details: { x: 1 } })) : Effect.sleep(Duration.millis(20)).pipe(Effect.andThen(Effect.die(new Error("bug")))),
    );
    r.call(1, "fail", null);
    r.call(2, "die", null);
    r.call(2, "dup", null);
    await until(() => sent.length === 3);
    expect(sent).toContainEqual({ t: "reply", id: 1, error: { code: "NOPE", message: "no", details: { x: 1 } } });
    expect(sent.find((m) => m.t === "reply" && m.id === 2 && m.error?.code === "INTERNAL")).toBeDefined();
    expect(sent.find((m) => m.t === "reply" && m.id === 2 && m.error?.code === "INVALID_REQUEST")).toBeDefined();
    const { r: bare, sent: none } = router(undefined);
    bare.call(5, "x.y", null);
    expect(none).toEqual([{ t: "reply", id: 5, error: { code: "INVALID_REQUEST", message: "no host call x.y", details: { method: "x.y" } } }]);
  });
});
