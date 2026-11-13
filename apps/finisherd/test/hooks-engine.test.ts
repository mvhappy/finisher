// The hook engine (7F plan Task 4; Decisions 1, 3, 4, 5, 8, 18, 21): the layers in order and
// each file in its order, the first block stops a blocking chain, context replies concatenate
// (64 KiB in all), a failure follows `onTimeout` (`PreMerge` fails closed), an unreadable layer
// blocks, `match` and `principals` filter, `PostCommand` and observe-only events run off the
// caller's path on one bounded queue whose hooks are resolved when the event runs, a block is
// audited without its reason, the counters `server.status` reports, and the cost contract: an
// event with no hook costs one map lookup and one `stat` per applying configuration file.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookEvent, Principal, Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { AuditInput } from "../src/control/audit.ts";
import type { ProjectTrust } from "../src/agent/rule-store.ts";
import { HookConfig } from "../src/hooks/config.ts";
import { buildHooks } from "../src/hooks/wiring.ts";
import { type HookAt, type HookNotice, type HookOutcome, makeHookEngine, type HookEngineDeps, type HookEngineImpl } from "../src/hooks/engine.ts";
import { AgentServer } from "./support/agent-server.ts";
import { ok } from "./support/ndjson-client.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });

let tmp: string;
const engines: HookEngineImpl[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-engine-")));
});
afterEach(async () => {
  for (const e of engines.splice(0)) await e.close();
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

const principal = (kind: Principal["kind"], id: string = kind): Principal => ({ kind, id, scopes: [], projects: "*", branches: "*", human: kind !== "agent" });
const AGENT = principal("agent", "s1");
const LOCAL = principal("local");
const SESSION = { id: "s1", projectId: "p1", harness: "fake", role: "editor", mode: "draft", rootBranch: "b1" } as unknown as Session;
const at = (over: Partial<HookAt> = {}): HookAt => ({ projectId: "p1", principal: AGENT, session: SESSION, ...over });

interface Fixture {
  readonly serverData: string;
  readonly dir: string;
  readonly config: HookConfig;
  readonly trusts: Map<string, ProjectTrust>;
  readonly audits: AuditInput[];
  readonly notices: Array<{ at: HookAt; notice: HookNotice }>;
  readonly delivered: Array<{ event: HookEvent; outcome: HookOutcome }>;
  readonly logs: string[];
}

function fixture(): Fixture {
  const serverData = join(tmp, "server-data");
  const dir = join(tmp, "P");
  mkdirSync(join(serverData, "projects", "p1"), { recursive: true });
  mkdirSync(dir, { recursive: true });
  copyFileSync(FAKE, join(dir, "fake-hook.mjs"));
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  copyFileSync(FAKE, join(serverData, "projects", "p1", "fake-hook.mjs"));
  const trusts = new Map<string, ProjectTrust>();
  const config = new HookConfig({ serverData, trustOf: (id) => trusts.get(id), dirOf: (id) => (id === "p1" ? dir : undefined) });
  return { serverData, dir, config, trusts, audits: [], notices: [], delivered: [], logs: [] };
}

function engine(f: Fixture, over: Partial<HookEngineDeps> = {}): HookEngineImpl {
  const e = makeHookEngine({
    config: f.config,
    serverData: f.serverData,
    audit: { append: (a) => void f.audits.push(a) },
    notices: { notice: (a, notice) => void f.notices.push({ at: a, notice }), deliver: (_a, event, outcome) => void f.delivered.push({ event, outcome }) },
    process: { status: DIRECT },
    log: (m) => void f.logs.push(m),
    ...over,
  });
  engines.push(e);
  return e;
}

/** The fake hook as configured in any layer: `run: ["node", "fake-hook.mjs", <args…>], file: "fake-hook.mjs"`. */
const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });
const ctx = (text: string) => ["--reply", `context:${text}`];
const block = (reason: string) => ["--reply", `block:${reason}`];
/** Writes `marker` (an absolute path) when it runs: proof that it started. */
const mark = (marker: string) => ["--write", marker];

const managed = (f: Fixture, hooks: unknown[]) => writeFileSync(join(f.serverData, "managed-settings.json"), JSON.stringify({ hooks }));
const user = (f: Fixture, hooks: unknown[]) => writeFileSync(join(f.serverData, "settings.json"), JSON.stringify({ hooks }));
const local = (f: Fixture, hooks: unknown[]) => writeFileSync(join(f.serverData, "projects", "p1", "hooks.json"), JSON.stringify({ hooks }));
function project(f: Fixture, hooks: unknown[], trust = true): void {
  writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks }));
  if (!trust) return;
  const hooksHash = f.config.list("p1").hooksHash;
  if (hooksHash === undefined) throw new Error("not trustable");
  f.trusts.set("p1", { projectId: "p1", rulesHash: "none", hooksHash, trustedAt: 0, by: "console:owner" });
}

const fire = (e: HookEngineImpl, event: HookEvent, a: HookAt = at(), payload: unknown = {}) => Effect.runPromise(e.fire(event, a, payload));

async function until<A>(f: () => A | undefined, ms = 10_000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() - t0 > ms) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** Waits until every file written so far is older than `RACY_MS` (its parsed value is then kept). */
const settle = () => new Promise((r) => setTimeout(r, 2_100));

describe("order and chain (Decision 4)", () => {
  test("layer_order_then_file_order: managed, user, project, local; context_concatenates_in_order", async () => {
    const f = fixture();
    managed(f, [hook("m1", "UserPrompt", ctx("managed-1"))]);
    user(f, [hook("u1", "UserPrompt", ctx("user-1")), hook("u2", "UserPrompt", ctx("user-2"))]);
    project(f, [hook("p1", "UserPrompt", ctx("project-1"))]);
    local(f, [hook("l1", "UserPrompt", ctx("local-1"))]);
    const out = await fire(engine(f), "UserPrompt");
    expect(out).toEqual({ ran: 5, context: "managed-1\n\nuser-1\n\nuser-2\n\nproject-1\n\nlocal-1", contextHooks: ["m1", "u1", "u2", "p1", "l1"] });
  });

  test("first_block_stops_chain: the third hook never starts", async () => {
    const f = fixture();
    const third = join(tmp, "third");
    user(f, [hook("a", "PreCommand", ctx("one")), hook("b", "PreCommand", block("not now")), hook("c", "PreCommand", mark(third))]);
    const e = engine(f);
    const out = await fire(e, "PreCommand");
    expect(out.blocked).toEqual({ hook: "b", reason: "not now", layer: "user" });
    expect(out.ran).toBe(2);
    expect(existsSync(third)).toBe(false);
    expect(e.stats().events.PreCommand).toEqual({ runs: 2, blocks: 1, timeouts: 0, failures: 0 });
  });

  test("context_over_64kib_drops_later_with_notice", async () => {
    const f = fixture();
    const big = join(f.serverData, "big.json");
    writeFileSync(big, `${JSON.stringify({ reply: "context", text: "x".repeat(16_000) })}\n`);
    user(f, [1, 2, 3, 4, 5].map((n) => hook(`c${n}`, "UserPrompt", ["--raw-file", big])));
    const out = await fire(engine(f), "UserPrompt");
    expect(out.ran).toBe(5);
    expect(out.contextHooks).toEqual(["c1", "c2", "c3", "c4"]);
    expect(Buffer.byteLength(out.context!)).toBe(4 * 16_000 + 3 * 2);
    expect(f.notices.map((n) => n.notice)).toEqual([{ hook: { event: "UserPrompt", id: "c5", layer: "user", contextDropped: 16_000 } }]);
  });

  test("an unreadable layer blocks every blocking event, and only those", async () => {
    const f = fixture();
    writeFileSync(join(f.serverData, "settings.json"), '{"hooks": [{"id": "x"}]}');
    const e = engine(f);
    const out = await fire(e, "PreMerge");
    expect(out.blocked).toEqual({ hook: "user.unreadable", reason: "unreadable", layer: "user" });
    expect(f.audits).toMatchObject([{ kind: "hook.block", outcome: "denied", details: { event: "PreMerge", hookId: "user.unreadable", layer: "user", projectId: "p1", session: "s1" } }]);
    expect((await fire(e, "SessionStart")).blocked).toBeUndefined();
    expect(e.stats().unreadable).toEqual(["user"]);
    // Fixed: it clears at once (a project's last view of the server layers does not keep it).
    user(f, []);
    expect(e.stats().unreadable).toEqual([]);
  });
});

describe("failures (Decision 3)", () => {
  test("failure_follows_on_timeout: allow passes with a notice, block is HOOK_BLOCKED {reason: failed}", async () => {
    const f = fixture();
    user(f, [hook("crash", "PreCommand", ["--stderr", `boom\u001b${"e".repeat(600)}`, "--exit", "3"])]);
    const e = engine(f);
    const passed = await fire(e, "PreCommand");
    expect(passed.blocked).toBeUndefined();
    const failed = f.notices.map((n) => n.notice);
    expect(failed).toHaveLength(1);
    expect(failed[0]).toMatchObject({ hook: { event: "PreCommand", id: "crash", layer: "user", failed: "exit", detail: "exit 3" } });
    // What it wrote on stderr reaches the notice bounded, control characters stripped.
    const stderr = (failed[0] as { hook: { stderr: string } }).hook.stderr;
    expect(stderr.startsWith("boomeee")).toBe(true);
    expect(Buffer.byteLength(stderr)).toBeLessThanOrEqual(512);
    user(f, [hook("crash", "PreCommand", ["--exit", "3"], { onTimeout: "block" })]);
    expect((await fire(e, "PreCommand")).blocked).toEqual({ hook: "crash", reason: "failed", layer: "user" });
    expect(e.stats().events.PreCommand).toMatchObject({ runs: 2, failures: 2, blocks: 1 });
  });

  test("pre_merge_timeout_blocks_by_default; timeout_notice_is_hook_timeout", { timeout: 20_000 }, async () => {
    const f = fixture();
    user(f, [hook("slow", "PreMerge", ["--sleep", "5000"], { timeoutS: 1 }), hook("slow-prompt", "UserPrompt", ["--sleep", "5000"], { timeoutS: 1 })]);
    const e = engine(f);
    expect((await fire(e, "PreMerge")).blocked).toEqual({ hook: "slow", reason: "timeout", layer: "user" });
    // Every other event fails open by default.
    expect((await fire(e, "UserPrompt")).blocked).toBeUndefined();
    expect(f.notices.map((n) => n.notice)).toEqual([
      { code: "HOOK_TIMEOUT", hook: "slow", event: "PreMerge", layer: "user", timeoutS: 1 },
      { code: "HOOK_TIMEOUT", hook: "slow-prompt", event: "UserPrompt", layer: "user", timeoutS: 1 },
    ]);
    expect(e.stats().events.PreMerge.timeouts).toBe(1);
  });

  test("an unconfined run warns once per server start, not per run", async () => {
    const f = fixture();
    user(f, [hook("a", "PreCommand", [])]);
    const e = engine(f);
    await fire(e, "PreCommand");
    await fire(e, "PreCommand");
    expect(f.logs.filter((l) => l.includes("SANDBOX_DISABLED"))).toHaveLength(1);
  });
});

describe("filters (Decisions 6, 15)", () => {
  test("match_filters_commands: any of the call's command names", async () => {
    const f = fixture();
    user(f, [hook("no-trim", "PreCommand", block("no trims"), { match: "item.*" })]);
    const e = engine(f);
    expect(await fire(e, "PreCommand", at({ names: ["view", "rpc:project.view"] }))).toEqual({ ran: 0 });
    expect((await fire(e, "PreCommand", at({ names: ["rpc:tx.commit", "item.trim"] }))).blocked?.hook).toBe("no-trim");
    // No names: a hook with a match does not run.
    expect(await fire(e, "PreCommand", at())).toEqual({ ran: 0 });
  });

  test("principals_agent_by_default: a person's call runs only principals: all", async () => {
    const f = fixture();
    user(f, [hook("agents", "PreCommand", block("agents only")), hook("everyone", "PreCommand", block("everyone"), { principals: "all" })]);
    const e = engine(f);
    expect((await fire(e, "PreCommand", at({ principal: AGENT }))).blocked?.hook).toBe("agents");
    expect((await fire(e, "PreCommand", ({ projectId: "p1", principal: LOCAL }))).blocked?.hook).toBe("everyone");
  });
});

describe("the observe queue (Decisions 1, 4, 18)", () => {
  test("observe_runs_after_the_call_answered", async () => {
    const f = fixture();
    const marker = join(tmp, "observed");
    user(f, [hook("later", "JobDone", ["--sleep", "300", ...mark(marker)])]);
    const e = engine(f);
    // An event with no hook first: the queue is not left stuck by it.
    e.observe("Notification", at(), {});
    await e.settled();
    const t0 = Date.now();
    e.observe("JobDone", at(), { kind: "media.look" });
    expect(Date.now() - t0).toBeLessThan(100);
    expect(existsSync(marker)).toBe(false);
    await e.settled();
    expect(existsSync(marker)).toBe(true);
  });

  test("block_on_observe_only_event_is_ignored_and_counted: later hooks still run", async () => {
    const f = fixture();
    user(f, [hook("nope", "JobDone", block("ignored")), hook("after", "JobDone", ctx("still runs"))]);
    const e = engine(f);
    e.observe("JobDone", at(), {});
    await e.settled();
    expect(f.audits).toEqual([]);
    expect(f.notices.map((n) => n.notice)).toEqual([{ hook: { event: "JobDone", id: "nope", layer: "user", ignoredBlock: "ignored" } }]);
    expect(f.delivered).toEqual([{ event: "JobDone", outcome: { ran: 2, context: "still runs", contextHooks: ["after"] } }]);
    expect(e.stats().events.JobDone).toEqual({ runs: 2, blocks: 1, timeouts: 0, failures: 0 });
  });

  test("post_command_block_stops_later_handlers: one hook.block {after: true}, the reason a notice", async () => {
    const f = fixture();
    const third = join(tmp, "third");
    user(f, [hook("a", "PostCommand", []), hook("b", "PostCommand", block("revert it")), hook("c", "PostCommand", mark(third))]);
    const e = engine(f);
    e.observe("PostCommand", at(), { rpc: "tx.commit" });
    await e.settled();
    expect(existsSync(third)).toBe(false);
    expect(f.audits).toHaveLength(1);
    expect(f.audits[0]).toMatchObject({ kind: "hook.block", principal: { kind: "agent", id: "s1" }, details: { event: "PostCommand", hookId: "b", layer: "user", after: true } });
    expect(f.notices.map((n) => n.notice)).toEqual([{ hook: { event: "PostCommand", id: "b", layer: "user", blocked: "revert it" } }]);
    expect(f.delivered).toEqual([{ event: "PostCommand", outcome: { ran: 2, blocked: { hook: "b", reason: "revert it", layer: "user" } } }]);
  });

  test("observe_queue_bounded: 256 wait, the next is dropped and counted, an earlier one runs", { timeout: 20_000 }, async () => {
    const f = fixture();
    const early = join(tmp, "early");
    const late = join(tmp, "late");
    const hooks = [hook("slow", "JobDone", ["--sleep", "500"], { match: "slow" }), hook("early", "JobDone", mark(early), { match: "early" }), hook("late", "JobDone", mark(late), { match: "late" })];
    user(f, [...hooks, hook("fast", "JobDone", [], { match: "fast" })]);
    const e = engine(f);
    e.observe("JobDone", at({ names: ["slow"] }), {});
    e.observe("JobDone", at({ names: ["early"] }), {});
    for (let i = 0; i < 255; i++) e.observe("JobDone", at({ names: ["fast"] }), {});
    e.observe("JobDone", at({ names: ["late"] }), {});
    expect(e.stats().dropped).toBe(1);
    // The fillers find no hook when they run (resolved then): the queue drains fast.
    user(f, hooks);
    await e.settled();
    expect(existsSync(early)).toBe(true);
    expect(existsSync(late)).toBe(false);
  });

  test("hookless events take no queue slot: 300 behind a slow hook drop nothing", async () => {
    const f = fixture();
    const later = join(tmp, "later");
    user(f, [hook("slow", "JobDone", ["--sleep", "500"], { match: "slow" }), hook("later", "JobDone", mark(later), { match: "later" })]);
    const e = engine(f);
    e.observe("JobDone", at({ names: ["slow"] }), {});
    for (let i = 0; i < 300; i++) e.observe("Notification", at({ names: ["status.idle"] }), {});
    e.observe("JobDone", at({ names: ["later"] }), {});
    expect(e.stats().dropped).toBe(0);
    await e.settled();
    expect(existsSync(later)).toBe(true);
  });

  test("queued_event_resolves_hooks_when_it_runs: a change while it waits applies to it", { timeout: 20_000 }, async () => {
    const f = fixture();
    const projectRan = join(tmp, "project-ran");
    const localRan = join(tmp, "local-ran");
    user(f, [hook("slow", "PostCommand", ["--sleep", "1500"], { match: "slow" })]);
    project(f, [hook("proj", "PostCommand", mark(projectRan), { match: "queued" })]);
    const e = engine(f);
    e.observe("PostCommand", at({ names: ["slow"] }), {});
    e.observe("PostCommand", at({ names: ["queued"] }), {});
    await new Promise((r) => setTimeout(r, 300));
    // While the second waits: the trusted project file changes (it no longer applies) and a local hook appears.
    writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [hook("proj", "PostCommand", mark(projectRan))] }));
    local(f, [hook("added", "PostCommand", mark(localRan), { match: "queued" })]);
    await e.settled();
    expect(existsSync(projectRan)).toBe(false);
    expect(existsSync(localRan)).toBe(true);
  });

  test("block_is_audited_without_reason, with the caller's principal", async () => {
    const f = fixture();
    user(f, [hook("guard", "PreCommand", block("secret reason"), { principals: "all" })]);
    await fire(engine(f), "PreCommand", ({ projectId: "p1", principal: LOCAL }));
    expect(f.audits).toEqual([{ kind: "hook.block", principal: { kind: "local", id: "local" }, outcome: "denied", code: "HOOK_BLOCKED", details: { event: "PreCommand", hookId: "guard", layer: "user", projectId: "p1" } }]);
    expect(JSON.stringify(f.audits)).not.toContain("secret reason");
  });
});

describe("freshness and cost (Decision 5)", () => {
  test("config_change_applies_without_restart", async () => {
    const f = fixture();
    user(f, [hook("a", "UserPrompt", ctx("first"))]);
    const e = engine(f);
    expect((await fire(e, "UserPrompt")).context).toBe("first");
    user(f, [hook("a", "UserPrompt", ctx("second")), hook("b", "UserPrompt", ctx("third"))]);
    expect((await fire(e, "UserPrompt")).context).toBe("second\n\nthird");
    rmSync(join(f.serverData, "settings.json"));
    expect(await fire(e, "UserPrompt")).toEqual({ ran: 0 });
  });

  test("no_hooks_costs_a_lookup_and_stats", { timeout: 60_000 }, async () => {
    // Load-sensitive (TASKS.md pattern): strict p99 bounds only with FIN_STRICT_TIMING=1.
    const strict = process.env.FIN_STRICT_TIMING === "1";
    const f = fixture();
    managed(f, [hook("m", "UserPrompt", [])]);
    user(f, [hook("u", "UserPrompt", [])]);
    local(f, [hook("l", "UserPrompt", [])]);
    writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [hook("p", "UserPrompt", [])] }));
    const stats: string[] = [];
    const stat = (p: string) => {
      stats.push(p);
      return statSync(p, { throwIfNoEntry: false });
    };
    const reads = vi.spyOn(f.config, "resolve");
    const e = engine(f, { stat });
    await settle();
    const runs = (n: number, a: HookAt) => {
      const ms: number[] = [];
      for (let i = 0; i < n; i++) {
        const t0 = performance.now();
        Effect.runSync(e.fire("PreMerge", a, {}));
        ms.push(performance.now() - t0);
      }
      return ms.sort((x, y) => x - y);
    };
    // Untrusted project: managed, user and local are checked, the project file is not.
    runs(1, at());
    expect(reads).toHaveBeenCalledTimes(1);
    stats.length = 0;
    const unchanged = runs(10_000, at());
    expect(reads).toHaveBeenCalledTimes(1);
    expect(stats.length).toBe(3 * 10_000);
    expect(new Set(stats)).toEqual(new Set([join(f.serverData, "managed-settings.json"), join(f.serverData, "settings.json"), join(f.serverData, "projects", "p1", "hooks.json")]));
    expect(e.stats().events.PreMerge).toEqual({ runs: 0, blocks: 0, timeouts: 0, failures: 0 });
    // Trusted: four locations per dispatch.
    project(f, [hook("p", "UserPrompt", [])]);
    runs(1, at());
    stats.length = 0;
    runs(100, at());
    expect(stats.length).toBe(4 * 100);
    // The changed path: each dispatch sees a new key and reads that file once.
    const user0 = join(f.serverData, "settings.json");
    const changed: number[] = [];
    reads.mockClear();
    for (let i = 0; i < 200; i++) {
      utimesSync(user0, new Date(), new Date(Date.now() - 10_000 - i * 1000));
      const t0 = performance.now();
      Effect.runSync(e.fire("PreMerge", at(), {}));
      changed.push(performance.now() - t0);
    }
    expect(reads).toHaveBeenCalledTimes(200);
    changed.sort((x, y) => x - y);
    const p99 = (ms: number[]) => ms[Math.floor(ms.length * 0.99)]!;
    console.log(`no-hook dispatch p50 ${unchanged[5000]!.toFixed(4)} ms, p99 ${p99(unchanged).toFixed(4)} ms; changed p50 ${changed[100]!.toFixed(4)} ms, p99 ${p99(changed).toFixed(4)} ms`);
    expect(p99(unchanged)).toBeLessThanOrEqual(strict ? 0.1 : 5);
    expect(p99(changed)).toBeLessThanOrEqual(strict ? 2 : 50);
  });
});

describe("the kept index and shutdown (review of Task 4)", () => {
  test("kept_index_sees_a_change: stats and the trusted hash key it", { timeout: 60_000 }, async () => {
    const f = fixture();
    const projectRan = join(tmp, "project-ran");
    user(f, [hook("u", "UserPrompt", [])]);
    local(f, [hook("l", "UserPrompt", [])]);
    project(f, [hook("p", "PreMerge", mark(projectRan))]);
    const resolve = vi.spyOn(f.config, "resolve");
    const applies = vi.spyOn(f.config, "applies");
    const e = engine(f);
    /** Waits past `RACY_MS`, then proves the index is kept (one resolve, then none). */
    const kept = async () => {
      await settle();
      await fire(e, "SessionStart");
      resolve.mockClear();
      await fire(e, "SessionStart");
      expect(resolve).not.toHaveBeenCalled();
    };
    await kept();
    // A user block hook appears: seen at once.
    user(f, [hook("u", "UserPrompt", []), hook("ub", "PreCommand", block("user says no"))]);
    expect((await fire(e, "PreCommand")).blocked?.hook).toBe("ub");
    await kept();
    local(f, [hook("l", "UserPrompt", []), hook("lb", "UserPrompt", block("local says no"))]);
    expect((await fire(e, "UserPrompt")).blocked?.hook).toBe("lb");
    // The trusted project file edited: its hook leaves the index (never even re-checked).
    await kept();
    writeFileSync(join(f.dir, "finisher.hooks.json"), JSON.stringify({ hooks: [hook("p", "PreMerge", mark(projectRan), { timeoutS: 2 })] }));
    applies.mockClear();
    expect(await fire(e, "PreMerge")).toEqual({ ran: 0 });
    expect(applies).not.toHaveBeenCalled();
    // Restored and trusted as it is, kept again; then the trust record names another hash.
    project(f, [hook("p", "PreMerge", mark(projectRan))]);
    await kept();
    const t = f.trusts.get("p1")!;
    f.trusts.set("p1", { ...t, hooksHash: `sha256:${"0".repeat(64)}` });
    applies.mockClear();
    expect(await fire(e, "PreMerge")).toEqual({ ran: 0 });
    expect(applies).not.toHaveBeenCalled();
    expect(existsSync(projectRan)).toBe(false);
  });

  test("close_kills_running_hook: its process and temp are gone", { timeout: 20_000 }, async () => {
    const f = fixture();
    const marker = join(tmp, "after-close");
    user(f, [hook("slow", "JobDone", ["--sleep", "1500", ...mark(marker)])]);
    const e = engine(f);
    e.observe("JobDone", at(), {});
    const runs = join(f.serverData, "hooks", "run");
    await until(() => (existsSync(runs) && readdirSync(runs).length > 0 ? true : undefined));
    await new Promise((r) => setTimeout(r, 300));
    await e.close();
    expect(readdirSync(runs)).toEqual([]);
    await new Promise((r) => setTimeout(r, 2_000));
    expect(existsSync(marker)).toBe(false);
  });

  test("fire_after_close_starts_nothing: PreMerge fails closed, others pass", async () => {
    const f = fixture();
    const marker = join(tmp, "ran");
    user(f, [hook("merge", "PreMerge", mark(marker)), hook("prompt", "UserPrompt", mark(marker))]);
    const e = engine(f);
    await e.close();
    expect((await fire(e, "PreMerge")).blocked).toEqual({ hook: "merge", reason: "failed", layer: "user" });
    expect(await fire(e, "UserPrompt")).toEqual({ ran: 0 });
    e.observe("JobDone", at(), {});
    await e.settled();
    expect(existsSync(marker)).toBe(false);
    expect(e.stats().events.PreMerge.runs).toBe(0);
  });

  test("stale_run_dirs_swept_at_start", () => {
    const f = fixture();
    const stale = join(f.serverData, "hooks", "run", "01STALE");
    mkdirSync(join(stale, ".hook", "0"), { recursive: true });
    writeFileSync(join(stale, ".hook", "0", "x.mjs"), "x");
    const w = buildHooks({ config: f.config, serverData: f.serverData, audit: { append: () => undefined } });
    engines.push(w.engine);
    expect(readdirSync(join(f.serverData, "hooks", "run"))).toEqual([]);
  });

  test("a rejected run is a failure under onTimeout, not a defect", async () => {
    const f = fixture();
    const config = new HookConfig({ serverData: f.serverData, trustOf: () => undefined, dirOf: () => f.dir, plugins: () => [{ plugin: "p", event: "PreMerge", handler: "h" }, { plugin: "p", event: "UserPrompt", handler: "u" }] });
    const e = engine(f, { config, plugins: { run: () => Promise.reject(new Error("plugin host gone")), digest: () => "d" } });
    expect((await fire(e, "PreMerge")).blocked).toEqual({ hook: "h", reason: "failed", layer: "plugin" });
    expect((await fire(e, "UserPrompt")).blocked).toBeUndefined();
    expect(f.notices.map((n) => n.notice)).toContainEqual({ hook: { event: "UserPrompt", id: "u", layer: "plugin", failed: "unavailable", detail: "plugin host gone" } });
  });
});

describe("server.status (Decision 21)", () => {
  test("server_status_reports_counters", { timeout: 60_000 }, async () => {
    const serverData = join(tmp, "server");
    mkdirSync(serverData, { recursive: true });
    copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: [hook("guard", "PreCommand", block("no"), { principals: "all" })] }));
    const server = new AgentServer(serverData, { hooks: { status: DIRECT } });
    await server.start();
    try {
      const out = await Effect.runPromise(server.agents.hookWiring.engine.fire("PreCommand", { projectId: "p0", principal: LOCAL }, {}));
      // No such project: its hooks fail (Decision 3) and fail open.
      expect(out.blocked).toBeUndefined();
      const status = ok<{ hooks: { events: Record<string, unknown>; dropped: number; unreadable: string[] } }>(await (await server.local()).call("server.status", {}));
      expect(status.hooks.events.PreCommand).toEqual({ runs: 0, blocks: 0, timeouts: 0, failures: 1 });
      expect(status.hooks.dropped).toBe(0);
      expect(status.hooks.unreadable).toEqual([]);
      expect(readFileSync(join(serverData, "settings.json"), "utf8")).toContain("guard");
    } finally {
      await server.stop();
    }
  });
});
