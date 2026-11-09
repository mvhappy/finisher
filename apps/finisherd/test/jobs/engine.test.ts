// The job engine: submit and cold runs (5C-1 plan Task 9a, Decisions 26, 27, 42, 43).
import { FinError } from "@finisher/contracts";
import { afterEach, describe, expect, test } from "vitest";
import { closeTestEngines, failure, OWNER, run, testClock, testEngine, until } from "../support/jobs.ts";

afterEach(closeTestEngines);

describe("submit", () => {
  test("unknown_kind", async () => {
    const t = await testEngine();
    const e = await failure(t.engine.submit(OWNER, { kind: "nope.kind", params: {} }));
    expect(e.code).toBe("JOB_KIND_UNKNOWN");
    expect(e.details).toEqual({ kind: "nope.kind" });
  });

  test("invalid_params_name_the_field", async () => {
    const t = await testEngine();
    const e = await failure(t.engine.submit(OWNER, { kind: "test.sleep", params: { ms: "soon" } }));
    expect(e.code).toBe("JOB_PARAMS_INVALID");
    expect((e.details as { errors: string[] }).errors.join(" ")).toContain("ms");
    expect(t.jobs.list().jobs).toHaveLength(0);
  });

  test("idem_returns_existing", async () => {
    const t = await testEngine();
    const a = await t.submit("test.sleep", { ms: 1 }, { idemKey: "k1" });
    const b = await t.submit("test.sleep", { ms: 1 }, { idemKey: "k1" });
    expect(b.id).toBe(a.id);
    const other = await run(t.engine.submit({ kind: "local", id: "someone-else" }, { kind: "test.sleep", params: { ms: 1 }, idemKey: "k1" }));
    expect(other.id).not.toBe(a.id);
  });

  test("dedupe_returns_active", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.sleep": { dedupe: () => "same" } } } });
    const a = await t.submit("test.sleep", { ms: 60_000 });
    const b = await t.submit("test.sleep", { ms: 60_000 });
    expect(b.id).toBe(a.id);
    expect((await run(t.engine.cancel(a.id))).state).toBe("cancelled");
    const c = await t.submit("test.sleep", { ms: 1 });
    expect(c.id).not.toBe(a.id);
    expect(c.dedupeKey).toBe("same");
  });

  test("queue_full_at_limit", async () => {
    const t = await testEngine({ queueLimit: 2 });
    await t.submit("test.sleep", { ms: 60_000 });
    await t.submit("test.sleep", { ms: 60_000 });
    const e = await failure(t.engine.submit(OWNER, { kind: "test.sleep", params: { ms: 1 } }));
    expect(e.code).toBe("JOB_QUEUE_FULL");
    expect(e.details).toEqual({ limit: 2 });
  });

  test("submit_dispatches_in_the_same_turn", async () => {
    const t = await testEngine();
    const v = await t.submit("test.sleep", { ms: 50 });
    expect(v.state).toBe("running");
    expect(v.attempts).toBe(1);
    expect(t.jobs.get(v.id)?.state).toBe("running");
  });

  test("an_undefined_pool_is_unavailable", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.sleep": { resources: ["io:nowhere"] } } } });
    const e = await failure(t.engine.submit(OWNER, { kind: "test.sleep", params: { ms: 1 } }));
    expect(e.code).toBe("WORKER_UNAVAILABLE");
    expect(e.details).toEqual({ pool: "io:nowhere" });
    t.engine.definePool("io:nowhere", 1);
    expect((await t.runJob("test.sleep", { ms: 1 })).state).toBe("succeeded");
  });

  test("jobs_wait_for_a_free_slot", async () => {
    const t = await testEngine({ cores: 2 });
    const a = await t.submit("test.sleep", { ms: 60_000 });
    const b = await t.submit("test.sleep", { ms: 1 });
    expect(b.state).toBe("queued");
    expect(t.jobs.get(b.id)?.state).toBe("queued");
    await run(t.engine.cancel(a.id));
    const [va, vb] = await run(t.engine.wait([a.id, b.id], { timeoutMs: 20_000 }));
    expect([va?.state, vb?.state]).toEqual(["cancelled", "succeeded"]);
    expect(vb!.startedAt!).toBeGreaterThanOrEqual(va!.finishedAt!);
  });

  test("get_list_and_wait_errors", async () => {
    const t = await testEngine();
    expect((await failure(t.engine.get("nope"))).code).toBe("JOB_NOT_FOUND");
    expect((await failure(t.engine.wait(["nope"], { timeoutMs: 10 }))).code).toBe("JOB_NOT_FOUND");
    const v = await t.submit("test.sleep", { ms: 2000 });
    expect((await failure(t.engine.wait([v.id], { timeoutMs: 50 }))).code).toBe("WAIT_TIMEOUT");
    expect(t.engine.list({}).jobs.map((j) => j.id)).toEqual([v.id]);
    expect(() => t.engine.list({ cursor: "bad" })).toThrow(FinError);
  });
});

describe("runs", () => {
  test("echo_succeeds_with_result", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.echo", { value: { n: 42 } });
    expect(v.state).toBe("succeeded");
    expect((v.result as { value: unknown }).value).toEqual({ n: 42 });
    expect(v.attempts).toBe(1);
    expect(t.engine.workers().processes).toHaveLength(0);
  });

  test("progress_events_are_throttled", async () => {
    // The engine's clock is a TestClock: only `adjust` lets a held progress event out.
    const tc = await testClock();
    const t = await testEngine({ clock: tc.clock });
    const v = await t.submit("test.sleep", { ms: 60_000, progressEveryMs: 5 });
    const progressEvents = () => t.events.filter((e) => e.type === "progress" && e.job.id === v.id);
    await until(() => progressEvents().length === 1);
    const first = progressEvents()[0]!.job.progress!.done;
    // More progress arrives (the view shows the newest) but no event leaves before 250 ms pass.
    await until(() => (t.engine.list({}).jobs[0]?.progress?.done ?? 0) > first);
    await new Promise((r) => setTimeout(r, 100));
    expect(progressEvents()).toHaveLength(1);
    await tc.adjust(250);
    await until(() => progressEvents().length === 2);
    expect(progressEvents()[1]!.job.progress!.done).toBeGreaterThan(first);
    await run(t.engine.cancel(v.id));
    expect((await run(t.engine.get(v.id))).progress).toBeUndefined();
    await t.close();
    await tc.close();
  });

  test("items_are_recorded", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.crash", { items: ["a", "b", "c"], how: "exit" });
    expect(v.state).toBe("succeeded");
    expect([...t.jobs.items(v.id).entries()]).toEqual([
      ["a", { value: { item: "a" } }],
      ["b", { value: { item: "b" } }],
      ["c", { value: { item: "c" } }],
    ]);
    expect(t.events.filter((e) => e.type === "item" && e.job.id === v.id).map((e) => (e as { item: string }).item)).toEqual(["a", "b", "c"]);
  });

  test("job_done_and_worker_exit_emitted", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.crash", { items: ["a", "b"], how: "exit" });
    expect(t.sink.done).toHaveLength(1);
    const done = t.sink.done[0]!;
    expect(done).toMatchObject({ kind: "test.crash", family: "test", outcome: "succeeded", attempts: 1, priority: "normal", warm: false, isolated: false, helpers: 0, items: 2, itemsFailed: 0 });
    expect(done.queuedMs).toBe(v.startedAt! - v.createdAt);
    expect(done.runMicros).toBeGreaterThan(0);
    expect(done.code).toBeUndefined();
    expect(t.sink.exits).toHaveLength(1);
    expect(t.sink.exits[0]).toMatchObject({ role: "worker", family: "test", reason: "done", exitCode: 0, jobsServed: 1, sandboxed: false });
    expect(t.sink.exits[0]!.lifetimeS).toBeGreaterThan(0);

    // Interactive with one attempt: neither retried nor isolated (Task 10).
    const failed = await t.runJob("test.crash", { items: ["x"], poison: ["x"], how: "abort" }, { priority: "interactive", maxAttempts: 1 });
    expect(failed.state).toBe("failed");
    expect(failed.error?.code).toBe("WORKER_CRASHED");
    expect(t.sink.done[1]).toMatchObject({ outcome: "failed", code: "WORKER_CRASHED", items: 1 });
    expect(t.sink.exits[1]).toMatchObject({ reason: "crashed", jobsServed: 1 });
  });

  test("owner_via_rights_are_kept", async () => {
    const t = await testEngine();
    const rights = { principal: OWNER, class: "person", scopes: ["project.read"], projects: "all", libraries: [], grants: [{ perm: "project:read" }] };
    const done: unknown[] = [];
    t.engine.onDone((j) => done.push(j));
    const v = await t.runJob("test.call", { method: "test.echo", params: { x: 1 } }, { via: { plugin: "p.one" }, rights });
    expect(v.state).toBe("succeeded");
    expect(v.result).toEqual({ echo: { x: 1 }, job: v.id, role: "worker", owner: OWNER, via: { plugin: "p.one" }, rights });
    expect(v).toMatchObject({ owner: OWNER, via: { plugin: "p.one" }, rights });
    expect(done).toEqual([v]);
  });

  test("a_throwing_on_done_listener_does_not_affect_the_job", async () => {
    const t = await testEngine();
    t.engine.onDone(() => {
      throw new Error("listener bug");
    });
    expect((await t.runJob("test.echo", { value: 1 })).state).toBe("succeeded");
  });

  test("diag_forwarded_only_when_on_and_allowed", async () => {
    const on = await testEngine({ diagnostics: () => true });
    const a = await on.runJob("test.diag", {});
    expect(a.result).toEqual({ sent: true });
    expect(on.sink.forwarded).toEqual([{ origin: { family: "test", job: a.id }, kind: "test.diag", payload: { n: 1 } }]);

    const off = await testEngine();
    const b = await off.runJob("test.diag", {});
    expect(b.result).toEqual({ sent: false });
    expect(off.sink.forwarded).toEqual([]);

    const notAllowed = await testEngine({ diagnostics: () => true, testKinds: { families: { test: { diagKinds: [] } } } });
    const c = await notAllowed.runJob("test.diag", {});
    expect(c.result).toEqual({ sent: true });
    expect(notAllowed.sink.forwarded).toEqual([]);
    expect(notAllowed.engine.workers().diagDropped).toBe(1);
  });

  test("protocol_mismatch_is_not_retried", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.badHello", {});
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("WORKER_PROTOCOL_MISMATCH");
    expect(v.attempts).toBe(1);
  });

  test("workers_reports_caps_and_pools", async () => {
    const t = await testEngine({ cores: 3 });
    const w = t.engine.workers();
    expect(w.caps.sandboxed).toBe(false);
    expect(w.pools.find((p) => p.name === "cpu")).toEqual({ name: "cpu", size: 2, used: 0, stuck: false });
    expect(w.warm).toEqual([]);
    await run(t.engine.warm("test"));
  });
});
