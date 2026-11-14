// `JobDone` (7F plan Task 8, Decision 18; spec §9.1) from the job engine's `onDone`: one
// observe-only event per finished job, `match` tested against the job kind, the project's own
// hooks only (a job of no project: the server layers only), never for a `plugin.hook` job (recursion) nor for a job a hook's own
// credential submitted; `PreRender`/`PostRender` are listed inert and nothing fires them
// (Decision 17). A real job engine (the test kinds) and a real hook engine (the fake hook,
// `direct`).
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookEvent, JobView } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { ProjectTrust } from "../src/agent/rule-store.ts";
import type { HookCredentialPort } from "../src/hooks/credential.ts";
import { HookConfig } from "../src/hooks/config.ts";
import { type HookEngineDeps, type HookEngineImpl, type HookOutcome, makeHookEngine } from "../src/hooks/engine.ts";
import { INERT_EVENTS, jobDoneHooks, PLUGIN_HOOK_KIND } from "../src/hooks/jobs.ts";
import { closeTestEngines, type TestEngine, testEngine } from "./support/jobs.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });

let tmp: string;
const engines: HookEngineImpl[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-jobs-")));
});
afterEach(async () => {
  for (const e of engines.splice(0)) await e.close();
  await closeTestEngines();
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });

interface Rig {
  readonly serverData: string;
  readonly dirs: Record<"p1" | "p2", string>;
  readonly config: HookConfig;
  readonly trusts: Map<string, ProjectTrust>;
  readonly hooks: HookEngineImpl;
  readonly delivered: Array<{ event: HookEvent; outcome: HookOutcome }>;
  readonly jobs: TestEngine;
  readonly listener: (job: JobView) => void;
}

async function rig(over: Partial<HookEngineDeps> = {}): Promise<Rig> {
  const serverData = join(tmp, "server-data");
  const dirs = { p1: join(tmp, "P1"), p2: join(tmp, "P2") };
  mkdirSync(serverData, { recursive: true });
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  for (const d of Object.values(dirs)) {
    mkdirSync(d, { recursive: true });
    copyFileSync(FAKE, join(d, "fake-hook.mjs"));
  }
  const trusts = new Map<string, ProjectTrust>();
  const config = new HookConfig({ serverData, trustOf: (id) => trusts.get(id), dirOf: (id) => (id === "p1" || id === "p2" ? dirs[id] : undefined) });
  const delivered: Rig["delivered"] = [];
  const hooks = makeHookEngine({
    config,
    serverData,
    audit: { append: () => undefined },
    notices: { notice: () => undefined, deliver: (_at, event, outcome) => void delivered.push({ event, outcome }) },
    process: { status: DIRECT },
    ...over,
  });
  engines.push(hooks);
  const jobs = await testEngine();
  const listener = jobDoneHooks({ engine: hooks });
  jobs.engine.onDone(listener);
  return { serverData, dirs, config, trusts, hooks, delivered, jobs, listener };
}

const user = (r: Rig, hooks: unknown[]) => writeFileSync(join(r.serverData, "settings.json"), JSON.stringify({ hooks }));
function project(r: Rig, p: "p1" | "p2", hooks: unknown[]): void {
  writeFileSync(join(r.dirs[p], "finisher.hooks.json"), JSON.stringify({ hooks }));
  const hooksHash = r.config.list(p).hooksHash;
  if (hooksHash === undefined) throw new Error("not trustable");
  r.trusts.set(p, { projectId: p, rulesHash: "none", hooksHash, trustedAt: 0, by: "console:owner" });
}

/** The request a `--report stdin` hook saw, from the context it answered. */
const seen = (o: HookOutcome) => JSON.parse((JSON.parse(o.context!) as { stdin: string }).stdin) as { event: string; project: { id: string }; payload: Record<string, unknown> };

describe("JobDone (Decision 18)", () => {
  test("job_done_fires_with_kind_match: the payload names the job; a hook matching another kind does not run", async () => {
    const r = await rig();
    const other = join(tmp, "other");
    user(r, [hook("echoes", "JobDone", ["--report", "stdin"], { match: "test.echo" }), hook("sleeps", "JobDone", ["--write", other], { match: "test.sleep" })]);
    const done = await r.jobs.runJob("test.echo", { value: 1 }, { project: "p1" });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
    expect(existsSync(other)).toBe(false);
    expect(r.delivered).toHaveLength(1);
    const req = seen(r.delivered[0]!.outcome);
    expect(req.event).toBe("JobDone");
    expect(req.project.id).toBe("p1");
    expect(req.payload).toEqual({ jobId: done.id, kind: "test.echo", kindVersion: done.kindVersion, state: done.state, project: "p1", owner: { kind: "local", id: "local" } });
  });

  test("a failed job's payload carries its error code only", async () => {
    const r = await rig();
    user(r, [hook("all", "JobDone", ["--report", "stdin"])]);
    const done = await r.jobs.runJob("test.fail", { code: "IO_ERROR", retry: "never" }, { project: "p1" });
    expect(done.state).toBe("failed");
    await r.hooks.settled();
    const req = seen(r.delivered[0]!.outcome);
    expect(req.payload.error).toEqual({ code: done.error!.code });
    expect(Object.keys(req.payload.error as object)).toEqual(["code"]);
  });

  test("job_done_project_hooks_see_own_project_only; a job of no project fires the server layers only", async () => {
    const r = await rig();
    const m1 = join(tmp, "p1-ran");
    project(r, "p1", [hook("mine", "JobDone", ["--write", m1])]);
    await r.jobs.runJob("test.echo", { value: 1 }, { project: "p2" });
    await r.hooks.settled();
    expect(existsSync(m1)).toBe(false);
    await r.jobs.runJob("test.echo", { value: 1 }, {});
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(0);
    // A user hook sees it, server-scoped: no project in its request or payload.
    user(r, [hook("server", "JobDone", ["--report", "stdin"])]);
    const done = await r.jobs.runJob("test.echo", { value: 1 }, {});
    await r.hooks.settled();
    expect(existsSync(m1)).toBe(false);
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
    const req = seen(r.delivered.at(-1)!.outcome);
    expect(req.project).toBeUndefined();
    expect(req.payload).toEqual({ jobId: done.id, kind: "test.echo", kindVersion: done.kindVersion, state: done.state, owner: { kind: "local", id: "local" } });
    user(r, []);
    await r.jobs.runJob("test.echo", { value: 1 }, { project: "p1" });
    await r.hooks.settled();
    expect(existsSync(m1)).toBe(true);
    expect(r.hooks.stats().events.JobDone.runs).toBe(2);
  });

  test("plugin_hook_jobs_fire_no_job_done", async () => {
    const r = await rig();
    user(r, [hook("all", "JobDone", [])]);
    const done = await r.jobs.runJob("test.echo", { value: 1 }, { project: "p1" });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
    // A `plugin.hook` job's end, as the engine would hand it over (the kind is Task 10's).
    expect(PLUGIN_HOOK_KIND).toBe("plugin.hook");
    r.listener({ ...done, kind: PLUGIN_HOOK_KIND, family: "plugin" });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
  });

  test("a job a hook's own credential submitted fires no JobDone (the recursion guard)", async () => {
    const isHook = (p: { kind: string; id: string }) => p.kind === "agent" && p.id === "hook-cred";
    const r = await rig({ credentials: { isHook, retain: () => undefined } as unknown as HookCredentialPort });
    user(r, [hook("all", "JobDone", [])]);
    const done = await r.jobs.runJob("test.echo", { value: 1 }, { project: "p1" });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
    r.listener({ ...done, owner: { kind: "agent", id: "hook-cred" } });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(1);
    r.listener({ ...done, owner: { kind: "agent", id: "a-session" } });
    await r.hooks.settled();
    expect(r.hooks.stats().events.JobDone.runs).toBe(2);
  });
});

describe("render events (Decision 17)", () => {
  test("render_events_listed_inert_and_never_fire", async () => {
    const r = await rig();
    const pre = join(tmp, "pre");
    const post = join(tmp, "post");
    user(r, [hook("pre", "PreRender", ["--write", pre]), hook("post", "PostRender", ["--write", post]), hook("done", "JobDone", [])]);
    expect(INERT_EVENTS).toEqual(["PreRender", "PostRender"]);
    const listed = r.config.list("p1").hooks.filter((h) => h.event === "PreRender" || h.event === "PostRender");
    expect(listed.map((h) => [h.id, h.inert])).toEqual([
      ["pre", "noRenderKind"],
      ["post", "noRenderKind"],
    ]);
    await r.jobs.runJob("test.echo", { value: 1 }, { project: "p1" });
    await r.hooks.settled();
    const s = r.hooks.stats().events;
    expect(s.JobDone.runs).toBe(1);
    expect(s.PreRender.runs + s.PostRender.runs).toBe(0);
    expect(existsSync(pre) || existsSync(post)).toBe(false);
  });
});
