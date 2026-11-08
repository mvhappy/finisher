// The bounds of the `jobs.*` RPCs (5C-2 review): open waits and watches per principal (M1),
// waiters woken by job (M1), `{all}` and `jobs.workers` only for every-project principals (M2),
// a project for limited principals and the kind's own project rules (M3), the per-owner limit,
// `interactive` for people only and the params and `idemKey` sizes (M4), the chain without the
// engine's view (L1), the `jobs.list` state filter (L3) and `jobs.watch`'s ids (L4).
import { IDEM_KEY_MAX, JOB_CALLS_MAX, JOB_PARAMS_MAX_BYTES, type JobView } from "@finisher/contracts";
import { Effect, Fiber, Stream } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import type { Engine } from "../src/jobs/engine.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { closeTestEngines, testEngine, until } from "./support/jobs.ts";
import { A, agent, code, err, rpc } from "./support/jobs-rpc.ts";

afterEach(() => closeTestEngines());

const engineOf = (e: unknown): Engine => e as Engine;

describe("jobs RPC limits", () => {
  test("open_waits_and_watches_are_bounded_per_principal", async () => {
    const t = await testEngine();
    const { as, slots } = rpc(t);
    const local = as(LOCAL);
    const job = await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 } });
    // Half waits, half watches: both count against the same bound.
    const waits = Array.from({ length: JOB_CALLS_MAX / 2 }, () => local.call("jobs.wait", { ids: [job.id], timeoutMs: 60_000 }));
    const watches = Array.from({ length: JOB_CALLS_MAX / 2 }, () =>
      Effect.runFork(Stream.runCollect(local.stream({ ids: [job.id] }).pipe(Stream.takeUntil((e) => e.job.finishedAt !== undefined)))),
    );
    await until(() => slots.open(LOCAL) === JOB_CALLS_MAX);
    expect(err(await local.call("jobs.wait", { ids: [job.id], timeoutMs: 60_000 }))).toMatchObject({ code: "TOO_MANY_WATCHES", details: { limit: JOB_CALLS_MAX } });
    expect(code(await Effect.runPromiseExit(Stream.runCollect(local.stream({ ids: [job.id] }))))).toBe("TOO_MANY_WATCHES");
    // Another principal has its own bound.
    const a1 = as(agent("01agent1"));
    const own = await a1.ok("jobs.submit", { kind: "test.echo", params: { value: 1 }, project: A });
    expect((await a1.ok<{ jobs: JobView[] }>("jobs.wait", { ids: [own.id], timeoutMs: 20_000 })).jobs[0]?.state).toBe("succeeded");
    // A refused call holds no slot; the job's end gives every slot back.
    expect(slots.open(LOCAL)).toBe(JOB_CALLS_MAX);
    await local.ok("jobs.cancel", { job: job.id });
    for (const w of await Promise.all(waits)) expect(code(w)).toBeUndefined();
    for (const f of watches) await Effect.runPromise(Fiber.join(f));
    await until(() => slots.open(LOCAL) === 0);
    // A timed-out wait gives its slot back too.
    const q = await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 } });
    expect(code(await local.call("jobs.wait", { ids: [q.id], timeoutMs: 20 }))).toBe("WAIT_TIMEOUT");
    expect(slots.open(LOCAL)).toBe(0);
    await local.ok("jobs.cancel", { job: q.id });
  }, 60_000);

  test("an_ending_job_wakes_only_its_own_waiters", async () => {
    const t = await testEngine();
    const e = engineOf(t.engine);
    const calls: string[] = [];
    e.addWaiter("a", () => calls.push("a"));
    const b = (): number => calls.push("b");
    e.addWaiter("b", b);
    e.wakeWaiters("a");
    expect(calls).toEqual(["a"]);
    e.unwait("b", b);
    expect(e.waiters.has("b")).toBe(false);
    // `wait` registers under each of its jobs, and leaves nothing behind once it resolves.
    const x = await t.submit("test.sleep", { ms: 60_000 });
    const y = await t.submit("test.echo", { value: 1 });
    const waiting = Effect.runPromise(t.engine.wait([x.id, y.id], { timeoutMs: 60_000, any: true }));
    await until(() => e.waiters.has(x.id) || (t.engine.list({ state: ["succeeded"] }).jobs.length === 1 && !e.waiters.has(y.id)));
    expect((await waiting).find((v) => v.id === y.id)?.state).toBe("succeeded");
    expect(e.waiters.has(x.id)).toBe(false);
    expect(e.waiters.has(y.id)).toBe(false);
    await Effect.runPromise(t.engine.cancel(x.id));
  }, 60_000);

  test("all_and_workers_need_every_project_in_reach", async () => {
    const t = await testEngine();
    const { as, audited } = rpc(t);
    const limited = as(agent("01ctl", { scopes: ["jobs.read", "jobs.submit", "jobs.control"] }));
    for (const [tag, payload] of [
      ["jobs.pause", { all: true }],
      ["jobs.resume", { all: true }],
      ["jobs.workers", {}],
    ] as const) {
      expect(err(await limited.call(tag, payload)), tag).toMatchObject({ code: "SCOPE_DENIED", details: { reason: "reach", scopes: [] } });
    }
    expect(audited.filter((e) => e.kind === "auth.denied" && (e.details as { reason?: string }).reason === "reach")).toHaveLength(3);
    // One job in reach stays the principal's to pause.
    const own = await limited.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 }, project: A });
    expect(code(await limited.call("jobs.pause", { job: own.id }))).toBeUndefined();
    // The same scopes over every project may.
    const everywhere = as(agent("01ctl2", { scopes: ["jobs.read", "jobs.control"], projects: "*" }));
    expect(await everywhere.ok("jobs.pause", { all: true })).toEqual({});
    expect(await everywhere.ok("jobs.resume", { all: true })).toEqual({});
    expect(await everywhere.ok<{ caps: unknown }>("jobs.workers", {})).toHaveProperty("caps");
    await as(LOCAL).ok("jobs.cancel", { job: own.id });
  }, 60_000);

  test("a_limited_principal_names_a_project_and_kinds_check_theirs", async () => {
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.echo": { requiresProject: true },
          "test.sleep": { paramsReach: (params: unknown) => ((params as { ms?: number }).ms === 7 ? ["ms 7 names another project"] : []) },
        },
      },
    });
    const { as } = rpc(t);
    const a1 = as(agent("01agent1"));
    expect(err(await a1.call("jobs.submit", { kind: "test.sleep", params: { ms: 1 } }))).toMatchObject({ code: "INVALID_REQUEST", details: { field: "project" } });
    expect((await a1.ok("jobs.submit", { kind: "test.sleep", params: { ms: 1 }, project: A })).project).toBe(A);
    // Every project in reach: no project needed, unless the kind needs one.
    const wide = as(agent("01agent2", { projects: "*" }));
    expect((await wide.ok("jobs.submit", { kind: "test.sleep", params: { ms: 1 } })).project).toBeUndefined();
    expect(err(await wide.call("jobs.submit", { kind: "test.echo", params: { value: 1 } }))).toMatchObject({ code: "INVALID_REQUEST", details: { field: "project" } });
    expect(err(await wide.call("jobs.submit", { kind: "test.sleep", params: { ms: 7 }, project: A }))).toMatchObject({
      code: "JOB_PARAMS_INVALID",
      details: { errors: ["ms 7 names another project"] },
    });
    await until(() => t.engine.list({ state: ["succeeded"] }).jobs.length === 2);
  }, 60_000);

  test("owners_are_limited_priority_is_a_persons_and_sizes_are_bounded", async () => {
    const t = await testEngine({ ownerLimit: 2 });
    const { as } = rpc(t);
    const a1 = as(agent("01agent1"));
    const local = as(LOCAL);
    const sleep = { kind: "test.sleep", params: { ms: 60_000 }, project: A };
    const [j1, j2] = [await a1.ok("jobs.submit", sleep), await a1.ok("jobs.submit", sleep)];
    expect(err(await a1.call("jobs.submit", sleep))).toMatchObject({ code: "JOB_QUEUE_FULL", details: { limit: 2, reason: "owner" } });
    // Another agent has its own limit; `local` and the console only the global one.
    const other = await as(agent("01agent2")).ok("jobs.submit", sleep);
    const mine = await Promise.all([1, 2, 3].map(() => local.ok("jobs.submit", sleep)));
    expect(engineOf(t.engine).owned.get("agent:01agent1")).toBe(2);
    expect(engineOf(t.engine).owned.has("local:local")).toBe(false);
    await a1.ok("jobs.cancel", { job: j1.id });
    const j3 = await a1.ok("jobs.submit", sleep);

    // `interactive` is a person's: an agent's (and `local`'s) submit gets the kind's default.
    const quick = { kind: "test.echo", params: { value: 1 }, priority: "interactive" };
    expect((await as(agent("01agent3")).ok("jobs.submit", { ...quick, project: A })).priority).toBe("normal");
    expect((await local.ok("jobs.submit", quick)).priority).toBe("normal");
    expect((await as(CONSOLE).ok("jobs.submit", quick)).priority).toBe("interactive");

    const big = "x".repeat(JOB_PARAMS_MAX_BYTES);
    expect(err(await local.call("jobs.submit", { kind: "test.echo", params: { value: big } }))).toMatchObject({ code: "JOB_PARAMS_INVALID", details: { max: JOB_PARAMS_MAX_BYTES } });
    expect(err(await local.call("jobs.submit", { kind: "test.echo", params: { value: 1 }, idemKey: "k".repeat(IDEM_KEY_MAX + 1) }))).toMatchObject({
      code: "INVALID_REQUEST",
      details: { field: "idemKey" },
    });
    expect((await local.ok("jobs.submit", { kind: "test.echo", params: { value: 1 }, idemKey: "k".repeat(IDEM_KEY_MAX) })).idemKey).toHaveLength(IDEM_KEY_MAX);
    for (const j of [j2, j3, other, ...mine]) await local.ok("jobs.cancel", { job: j.id });
    // agent3's echo may still be running: each count drops as its jobs finish.
    await until(() => engineOf(t.engine).owned.size === 0);
  }, 60_000);

  test("list_states_and_watch_ids_are_bounded_and_the_chain_needs_the_engine", async () => {
    const t = await testEngine();
    const local = rpc(t).as(LOCAL);
    expect(err(await local.call("jobs.list", { state: Array(9).fill("queued") }))).toMatchObject({ code: "INVALID_REQUEST", details: { field: "state", max: 8 } });
    expect((await local.ok<{ jobs: JobView[] }>("jobs.list", { state: ["queued", "queued", "running"] })).jobs).toEqual([]);
    expect(err(await Effect.runPromiseExit(Stream.runCollect(local.stream({ ids: [] }))))).toMatchObject({ code: "INVALID_REQUEST", details: { field: "ids" } });
    // A jobs rule without the engine's view is refused, never let through as `none` (L1).
    const blind = rpc(t, { jobs: undefined }).as(LOCAL);
    expect(code(await blind.call("jobs.get", { job: "01zzzzzzzzzzzzzzzzzzzzzzzz" }))).toBe("INTERNAL");
    expect(code(await blind.call("jobs.submit", { kind: "test.echo", params: { value: 1 } }))).toBe("INTERNAL");
    expect(code(await blind.call("jobs.list", {}))).toBeUndefined();
  }, 60_000);
});
