// The `jobs.*` RPCs (5C-2 plan Task 1) through the real chain over a real job engine with the
// test kinds: scopes, ownership, reach, kind scopes and confirmation, dedupe per owner,
// `jobs.watch` and its lagging bound, and `server.status`'s job caps.
import type { JobView } from "@finisher/contracts";
import { Duration, Effect, Fiber, Stream } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { agentWarnings } from "../src/agent/warnings.ts";
import type { Audit } from "../src/control/audit.ts";
import { canSee } from "../src/jobs/access.ts";
import { watchJobs } from "../src/jobs/watch.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { sandboxWarning, serverStatus } from "../src/server-admin.ts";
import { closeTestEngines, testEngine, until } from "./support/jobs.ts";
import { A, agent, B, code, err, normalised, rpc } from "./support/jobs-rpc.ts";

afterEach(() => closeTestEngines());

describe("jobs RPCs", () => {
  test("ownership_and_reach: others' jobs and jobs out of reach answer as unknown ones", async () => {
    const t = await testEngine();
    const { as } = rpc(t);
    const local = as(LOCAL);
    const a1 = as(agent("01agent1"));
    const a2 = as(agent("01agent2", { projects: "*" }));
    const control = as(agent("01control", { scopes: ["jobs.read", "jobs.control"] }));

    const mine = await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 }, project: A });
    expect(mine.owner).toEqual({ kind: "local", id: "local" });
    const inB = await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 }, project: B });
    const unknown = "01zzzzzzzzzzzzzzzzzzzzzzzz";

    // Another owner's job is JOB_NOT_FOUND exactly as an unknown id is.
    const other = err(await a1.call("jobs.get", { job: mine.id }));
    const none = err(await a1.call("jobs.get", { job: unknown }));
    expect(other?.code).toBe("JOB_NOT_FOUND");
    expect(normalised(other, mine.id)).toEqual(normalised(none, unknown));
    expect(other?.details).toEqual({ job: mine.id });
    for (const tag of ["jobs.cancel", "jobs.pause", "jobs.resume"]) expect(code(await a1.call(tag, { job: mine.id })), tag).toBe("JOB_NOT_FOUND");
    expect(code(await a1.call("jobs.wait", { ids: [mine.id], timeoutMs: 10 }))).toBe("JOB_NOT_FOUND");

    // `jobs.control` sees every owner's job in its reach, and nothing outside it.
    expect((await control.ok("jobs.get", { job: mine.id })).id).toBe(mine.id);
    expect(code(await control.call("jobs.get", { job: inB.id }))).toBe("JOB_NOT_FOUND");
    const listed = await control.ok<{ jobs: JobView[] }>("jobs.list", {});
    expect(listed.jobs.map((j) => j.id)).toEqual([mine.id]);

    // A project named out of reach is the chain's PROJECT_NOT_FOUND.
    expect(code(await a1.call("jobs.submit", { kind: "test.echo", params: { value: 1 }, project: B }))).toBe("PROJECT_NOT_FOUND");
    expect(code(await a1.call("jobs.list", { project: B }))).toBe("PROJECT_NOT_FOUND");

    // An agent's own jobs only; the owner sees everything (local holds jobs.control), or only its own with `mine`.
    const own = await a1.ok("jobs.submit", { kind: "test.echo", params: { value: 1 }, project: A });
    expect((await a1.ok<{ jobs: JobView[] }>("jobs.list", {})).jobs.map((j) => j.id)).toEqual([own.id]);
    expect((await a2.ok<{ jobs: JobView[] }>("jobs.list", {})).jobs).toEqual([]);
    expect((await local.ok<{ jobs: JobView[] }>("jobs.list", {})).jobs.map((j) => j.id).sort()).toEqual([mine.id, inB.id, own.id].sort());
    expect((await local.ok<{ jobs: JobView[] }>("jobs.list", { mine: true })).jobs.map((j) => j.id).sort()).toEqual([mine.id, inB.id].sort());
    const [done] = (await a1.ok<{ jobs: JobView[] }>("jobs.wait", { ids: [own.id], timeoutMs: 20_000 })).jobs;
    expect(done?.state).toBe("succeeded");

    // Acting on one's own job takes `jobs.submit`; on another's, `jobs.control` (no `jobs.submit` needed).
    const reader = as(agent("01agent1", { scopes: ["jobs.read"] }));
    expect(err(await reader.call("jobs.cancel", { job: own.id }))).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["jobs.submit"] } });
    expect((await control.ok("jobs.cancel", { job: mine.id })).state).toBe("cancelled");
    expect(err(await a1.call("jobs.pause", { all: true }))).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["jobs.control"] } });
    expect(code(await a1.call("jobs.workers", {}))).toBe("SCOPE_DENIED");
    expect((await local.ok<{ caps: { sandboxed: boolean } }>("jobs.workers", {})).caps.sandboxed).toBe(false);
    expect(code(await local.call("jobs.pause", {}))).toBe("INVALID_REQUEST");
    expect(code(await local.call("jobs.pause", { job: inB.id, all: true }))).toBe("INVALID_REQUEST");
    expect(code(await local.call("jobs.list", { limit: 0 }))).toBe("INVALID_REQUEST");
    expect(code(await local.call("jobs.wait", { ids: [inB.id], timeoutMs: 600_001 }))).toBe("INVALID_REQUEST");
    expect(code(await local.call("jobs.wait", { ids: [inB.id], timeoutMs: 50 }))).toBe("WAIT_TIMEOUT");
    await local.ok("jobs.cancel", { job: inB.id });
  }, 60_000);

  test("pause_and_resume_one_job_and_all", async () => {
    const t = await testEngine();
    const local = rpc(t).as(LOCAL);
    expect(await local.ok("jobs.pause", { all: true })).toEqual({});
    const q = await local.ok("jobs.submit", { kind: "test.echo", params: { value: 2 } });
    expect(q.state).toBe("queued");
    const paused = await local.ok<{ job: JobView }>("jobs.pause", { job: q.id });
    expect(paused.job.state).toBe("paused");
    expect((await local.ok<{ job: JobView }>("jobs.resume", { job: q.id })).job.state).toBe("queued");
    expect(await local.ok("jobs.resume", { all: true })).toEqual({});
    expect((await t.wait(q.id)).state).toBe("succeeded");
  }, 60_000);

  test("library_jobs_need_library_read", async () => {
    const t = await testEngine();
    const lib = await Effect.runPromise(t.engine.submit({ kind: "agent", id: "01agent1" }, { kind: "test.echo", params: { value: 1 }, library: "lib1" }));
    const { as } = rpc(t);
    expect((await as(agent("01agent1")).ok("jobs.get", { job: lib.id })).id).toBe(lib.id);
    const blind = as(agent("01agent1", { scopes: ["jobs.read", "jobs.submit"] }));
    expect(code(await blind.call("jobs.get", { job: lib.id }))).toBe("JOB_NOT_FOUND");
    expect((await blind.ok<{ jobs: JobView[] }>("jobs.list", {})).jobs).toEqual([]);
    expect(canSee(agent("01agent1"), { owner: { kind: "agent", id: "01agent1" }, project: B })).toBe(false);
  }, 60_000);

  test("kind_scopes_join_jobs_submit_and_only_rpc_kinds_are_submittable", async () => {
    const t = await testEngine({
      testKinds: { overrides: { "test.hang": { scopes: ["library.scan"] }, "test.stall": { scopes: ["library.admin"] }, "test.fail": { rpc: false } } },
    });
    const { as, audited } = rpc(t);
    const noScan = as(agent("01agent1", { scopes: ["jobs.read", "jobs.submit"] }));
    expect(err(await noScan.call("jobs.submit", { kind: "test.hang", params: {} }))).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["library.scan"] } });
    expect(audited.some((e) => e.kind === "auth.denied" && (e.details as { rpc?: string }).rpc === "jobs.submit")).toBe(true);
    // A human-only kind scope: refused for agents and `local`, confirmed by default for the console.
    expect(code(await as(agent("01agent1")).call("jobs.submit", { kind: "test.stall", params: {} }))).toBe("HUMAN_ONLY");
    expect(code(await as(LOCAL).call("jobs.submit", { kind: "test.stall", params: {} }))).toBe("HUMAN_ONLY");
    expect(err(await as(CONSOLE).call("jobs.submit", { kind: "test.stall", params: {} }))).toMatchObject({ code: "CONFIRMATION_REQUIRED" });
    // A kind that did not opt in answers as an unknown one.
    const notRpc = err(await as(LOCAL).call("jobs.submit", { kind: "test.fail", params: {} }));
    const unknown = err(await as(LOCAL).call("jobs.submit", { kind: "test.nope", params: {} }));
    expect(notRpc?.code).toBe("JOB_KIND_UNKNOWN");
    expect(unknown?.code).toBe("JOB_KIND_UNKNOWN");
    expect(notRpc?.message.replace("test.fail", "K")).toBe(unknown?.message.replace("test.nope", "K"));
  }, 60_000);

  test("dedupe_is_per_owner", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.sleep": { dedupe: () => "same" } } } });
    const { as } = rpc(t);
    const local = as(LOCAL);
    const a = await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 } });
    expect((await local.ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 } })).id).toBe(a.id);
    const b = await as(agent("01agent1")).ok("jobs.submit", { kind: "test.sleep", params: { ms: 60_000 }, project: A });
    expect(b.id).not.toBe(a.id);
    expect(b.owner).toEqual({ kind: "agent", id: "01agent1" });
    for (const id of [a.id, b.id]) await local.ok("jobs.cancel", { job: id });
  }, 60_000);

  test("watch_sends_only_visible_jobs_and_the_watched_ones_first", async () => {
    const t = await testEngine();
    const { as } = rpc(t);
    const a1 = as(agent("01agent1"));
    const local = as(LOCAL);
    const all = Effect.runFork(Stream.runCollect(a1.stream({}).pipe(Stream.takeUntil((e) => e.type === "state" && e.job.state === "succeeded"))));
    await new Promise((r) => setTimeout(r, 20));
    await local.ok("jobs.submit", { kind: "test.echo", params: { value: "theirs" }, project: A });
    const own = await a1.ok("jobs.submit", { kind: "test.echo", params: { value: "mine" }, project: A });
    const seen = await Effect.runPromise(Fiber.join(all));
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((e) => e.job.id === own.id)).toBe(true);

    // With ids: the current view first, then that job's events only.
    const slow = await a1.ok("jobs.submit", { kind: "test.sleep", params: { ms: 300 }, project: A });
    const one = await Effect.runPromise(Stream.runCollect(a1.stream({ ids: [slow.id] }).pipe(Stream.takeUntil((e) => e.job.finishedAt !== undefined))));
    expect(one[0]).toMatchObject({ type: "state", job: { id: slow.id } });
    expect(one.at(-1)?.job.state).toBe("succeeded");
    const exit = await Effect.runPromiseExit(Stream.runCollect(a1.stream({ ids: [own.id, "01zzzzzzzzzzzzzzzzzzzzzzzz"] })));
    expect(code(exit)).toBe("JOB_NOT_FOUND");
    const theirs = (await local.ok<{ jobs: JobView[] }>("jobs.list", { mine: true })).jobs[0]!;
    expect(code(await Effect.runPromiseExit(Stream.runCollect(a1.stream({ ids: [theirs.id] }))))).toBe("JOB_NOT_FOUND");
  }, 60_000);

  test("a_watcher_that_falls_behind_ends_watch_lagging_and_others_carry_on", async () => {
    const t = await testEngine();
    const view = (id: string) => Effect.runSync(Effect.option(t.engine.get(id))).pipe((o) => (o._tag === "Some" ? o.value : undefined));
    // A reader that sleeps on its first event, with room for 4 more.
    const slow = Effect.runFork(Stream.runCollect(watchJobs(t.engine, view, LOCAL, {}, 4).pipe(Stream.tap(() => Effect.sleep(Duration.millis(300))))));
    const fast = Effect.runFork(Stream.runCollect(watchJobs(t.engine, view, LOCAL, {}, 4096).pipe(Stream.take(20))));
    await new Promise((r) => setTimeout(r, 20));
    for (let i = 0; i < 10; i++) await t.submit("test.echo", { value: i });
    const lagged = await Effect.runPromiseExit(Fiber.join(slow));
    expect(err(lagged)).toMatchObject({ code: "WATCH_LAGGING", details: { buffer: 4 } });
    expect((await Effect.runPromise(Fiber.join(fast))).length).toBe(20);
    await until(() => t.engine.list({ state: ["succeeded"] }).jobs.length === 10);
  }, 60_000);
});

describe("server.status", () => {
  const caps = { killTree: "processGroup", watchdog: true, cgroupMemory: false, leafLimits: false, sandbox: "none", sandboxed: false } as const;
  const deps = (c: typeof caps | (Omit<typeof caps, "sandboxed" | "sandbox"> & { sandboxed: true; sandbox: string })) => ({
    control: { envId: "env", listeners: () => [], stop: () => {} },
    audit: { failures: () => 0 } as unknown as Audit,
    connections: { count: () => ({}) } as never,
    journalEnabled: () => false,
    jobCaps: () => c as never,
  });

  test("reports_the_job_hosts_caps_and_warns_while_unsandboxed", async () => {
    const s = await Effect.runPromise(serverStatus(deps(caps)));
    expect(s.jobs.caps).toEqual(caps);
    // The job host's warning, then the agent runtime's (agent tools are unsandboxed until 7C).
    expect(s.warnings).toEqual([sandboxWarning(caps as never), ...agentWarnings()]);
    expect(s.warnings[0]).toMatchObject({ code: "SANDBOX_DISABLED", sandbox: "none" });
    const boxed = await Effect.runPromise(serverStatus(deps({ ...caps, sandboxed: true, sandbox: "fin-sandbox" })));
    expect(boxed.warnings).toEqual(agentWarnings());
  });
});
