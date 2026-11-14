// The agent runtime engine (plan 7A Task 6b; Decisions 2, 3, 5, 6, 21), driven by the fake provider.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import type { FinError, Node, WatchItem } from "@finisher/contracts";
import { Effect, Fiber, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { AgentStore, SERVER_RESTART } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { credentialPath } from "../src/agent/credentials.ts";
import { FinTool, makeRuntime, META, PEOPLE, TEST_PRICING } from "./support/runtime-doubles.ts";

const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
let dir: string;
let control: ControlStore;
let clock: ReturnType<typeof manualClock>;
let store: AgentStore;
const cleanups: (() => Promise<void>)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-runtime-"));
  clock = manualClock(Date.now());
  control = openControlStore(dir, { serverId: "sid", checkpoint: false });
  store = new AgentStore(control, dir, clock, systemIds(clock));
});

afterEach(async () => {
  for (const c of cleanups.splice(0)) await c();
  await control.close();
  rmSync(dir, { recursive: true, force: true });
});

const fixture = (name: string): FakeScript =>
  JSON.parse(readFileSync(new URL(`./fixtures/fake-scripts/${name}.json`, import.meta.url), "utf8")) as FakeScript;

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = async (e: Effect.Effect<unknown, FinError>) => Effect.runPromise(Effect.flip(e));

async function until(f: () => boolean, what = "condition", ms = 5000): Promise<void> {
  const t0 = Date.now();
  while (!f()) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 5));
  }
}

function harness(adapter: ReturnType<typeof FakeAdapter>, opts: Parameters<typeof makeRuntime>[2] = {}) {
  const h = makeRuntime({ store, control }, adapter, { clock, ...opts });
  cleanups.push(() => h.runtime.close());
  return h;
}

function watch(h: ReturnType<typeof harness>, sessionId: string, fromSeq?: number) {
  const items: WatchItem[] = [];
  const fiber = Effect.runFork(Stream.runForEach(h.runtime.watch(sessionId, fromSeq), (i) => Effect.sync(() => void items.push(i))));
  cleanups.push(() => Effect.runPromise(Fiber.interrupt(fiber)));
  return { items, fiber, nodes: () => items.flatMap((i) => (i.type === "nodeAdded" ? [i.node] : [])) };
}

const status = (id: string) => store.get(id)!.status;
const credential = (h: ReturnType<typeof harness>, id: string) => h.core.auth.get(id)?.status;
const kinds = (id: string) => store.nodes(id).map((n) => n.kind);
const payload = (n: Node) => n.payload as Record<string, unknown>;
const idle = (id: string) => () => status(id) === "idle";

const toolTurn = (...calls: [string, string[]][]) =>
  calls.flatMap(([id, argv]) => [{ emit: [{ type: "toolStarted", providerId: id, tool: "fin", input: { argv } }] }, { awaitTool: id }]);

describe("agent runtime", () => {
  test("events_become_nodes_in_order", async () => {
    const h = harness(FakeAdapter([fixture("simple")]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    expect(session).toMatchObject({ status: "idle", rootBranch: "br1", base: { branch: "main", seq: 3 } });
    // Task 7 opened it: the root branch created as the starter, the credential minted with its id.
    expect(h.registry.created).toEqual([{ projectId: "P1", name: "agent.editor-1", from: "main", author: `agent:editor:${session.id}`, meta: META }]);
    expect(credential(h, session.id)).toBe("live");
    expect(existsSync(credentialPath(store.root, session.id))).toBe(true);
    const w = watch(h, session.id);
    const r = await run(h.runtime.prompt(session.id, "tighten the intro", PEOPLE));
    expect(r).toMatchObject({ seq: 1, trigger: "prompt", headAtStart: 3 });
    await until(idle(session.id), "idle");
    expect(kinds(session.id)).toEqual(["userMessage", "reasoning", "assistantMessage", "usage"]);
    expect(store.run(r.id)).toMatchObject({ status: "completed", stopReason: "endTurn", usage: { input: 1200, output: 40, cacheWrite: 900 } });
    expect(store.get(session.id)!.usage).toMatchObject({ input: 1200, output: 40, costBasis: "unknown" });
    // Deltas are streamed, not stored; every node reaches the watcher once, in order.
    await until(() => w.nodes().length === 4, "watched nodes");
    expect(w.items.filter((i) => i.type === "delta")).toEqual([{ type: "delta", runId: r.id, kind: "message", text: "Looking" }]);
    expect(w.nodes().map((n) => n.seq)).toEqual([1, 2, 3, 4]);
    expect(w.items.filter((i) => i.type === "statusChanged").map((i) => i.type === "statusChanged" && i.status)).toEqual(["idle", "running", "idle"]);
    expect(w.items.some((i) => i.type === "usageUpdated")).toBe(true);
    expect(h.journal.entries.filter((e) => e.kind === "session").map((e) => e.phase)).toEqual(["started"]);
    expect(h.journal.entries.filter((e) => e.kind === "turn")).toHaveLength(1);
    expect(store.outbox()).toEqual([]);
  });

  test("tool_call_gets_a_ulid_call_id", async () => {
    const h = harness(FakeAdapter([fixture("tool")]));
    await h.runtime.boot();
    const seen: string[] = [];
    // The call is announced (its node stored) before the tool runs.
    h.tool.before = async (c) => {
      const call = store.nodes(c.sessionId).find((n) => n.kind === "toolCall" && n.callId === c.callId);
      seen.push(call ? `${call.providerId}:${c.callId}` : "missing");
      return "continue";
    };
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim it" }, PEOPLE, META));
    await until(idle(session.id), "idle");
    const nodes = store.nodes(session.id);
    const calls = nodes.filter((n) => n.kind === "toolCall");
    expect(calls.map((n) => n.providerId)).toEqual(["call_1", "call_2"]);
    for (const c of calls) expect(c.callId).toMatch(ULID);
    expect(seen).toEqual(calls.map((c) => `${c.providerId}:${c.callId}`));
    expect(h.tool.calls.map((c) => c.callId)).toEqual(calls.map((c) => c.callId));
    expect(h.tool.calls[0]).toMatchObject({ cwd: `/workspace/${session.id}`, env: { FIN_SESSION: session.id, FIN_CREDENTIAL: credentialPath(store.root, session.id) } });
    const results = nodes.filter((n) => n.kind === "toolResult");
    expect(results.map((n) => n.callId)).toEqual(calls.map((c) => c.callId));
    expect(payload(results[1]!)).toEqual({ ok: true, output: { txId: `tx-${calls[1]!.callId}` } });
    expect(kinds(session.id)).toEqual(["userMessage", "toolCall", "toolResult", "toolCall", "toolResult", "assistantMessage", "usage"]);
    expect(store.get(session.id)!.usage).toMatchObject({ moneyMicros: 120, costBasis: "reported" });
    expect(h.journal.entries.filter((e) => e.kind === "toolResult").map((e) => e.synthetic)).toEqual([false, false]);
  });

  test("late_events_from_replaced_instance_dropped", async () => {
    const adapter = FakeAdapter([fixture("crash-late"), fixture("simple")]);
    const h = harness(adapter, { tools: {} });
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    await until(() => status(session.id) === "interrupted", "interrupted");
    expect(store.get(session.id)!.statusReason).toBe("PROVIDER_EXITED");
    expect(kinds(session.id)).toEqual(["userMessage", "toolCall", "error", "toolResult"]);
    expect(payload(store.nodes(session.id)[3]!)).toMatchObject({ code: "SESSION_INTERRUPTED", synthetic: true });
    const w = watch(h, session.id);
    await run(h.runtime.resume(session.id, PEOPLE, "again"));
    await until(idle(session.id), "idle");
    await until(() => h.runtime.stats().dropped >= 2, "late events dropped");
    expect(adapter.log.opened.map((o) => o.instance)).toEqual(["fake-1", "fake-2"]);
    // The stale result and the old delta never became a node or a watch item.
    const all = store.nodes(session.id);
    expect(all.filter((n) => n.kind === "toolResult")).toHaveLength(1);
    expect(JSON.stringify(all)).not.toContain("stale");
    expect(JSON.stringify(w.items)).not.toContain("from the old process");
    expect(h.journal.entries.filter((e) => e.kind === "session").map((e) => e.phase)).toEqual(["started", "interrupted", "resumed"]);
    expect(credential(h, session.id)).toBe("live");
  });

  test("unknown_event_becomes_notice", async () => {
    const h = harness(FakeAdapter([fixture("unknown")]));
    await h.runtime.boot();
    const { session, run: r } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(idle(session.id), "idle");
    expect(kinds(session.id)).toEqual(["userMessage", "notice", "assistantMessage"]);
    expect(payload(store.nodes(session.id)[1]!)).toEqual({ unknownEvent: "rateLimitUpdate" });
    expect(store.run(r!.id)).toMatchObject({ status: "completed", stopReason: "endTurn" });
  });

  test("a_notice_event_becomes_a_notice_node", async () => {
    const script: FakeScript = { turns: [[{ emit: [{ type: "notice", name: "retry", details: { attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 1000 } }, { type: "message", role: "assistant", blocks: [{ type: "text", text: "ok" }] }] }]] };
    const h = harness(FakeAdapter([script]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(idle(session.id), "idle");
    expect(kinds(session.id)).toEqual(["userMessage", "notice", "assistantMessage"]);
    expect(payload(store.nodes(session.id)[1]!)).toEqual({ retry: { attempt: 1, code: "PROVIDER_OVERLOADED", delayMs: 1000 } });
  });

  test("spawn_failure_fails_turn_provider_exited", async () => {
    const h = harness(FakeAdapter([fixture("spawn-fail")]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    const e = await failure(h.runtime.prompt(session.id, "go", PEOPLE));
    expect(e.code).toBe("PROVIDER_EXITED");
    expect(store.get(session.id)).toMatchObject({ status: "interrupted", statusReason: "PROVIDER_EXITED" });
    expect(store.runs(session.id)).toMatchObject([{ status: "interrupted", stopReason: "PROVIDER_EXITED" }]);
    expect(kinds(session.id)).toEqual(["userMessage", "error"]);
    expect(h.runtime.stats().live).toBe(0);
  });

  test("interrupt_keeps_branch_and_status_interrupted", async () => {
    const adapter = FakeAdapter([fixture("tool")]);
    const h = harness(adapter);
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1, "the tool running");
    const s = await run(h.runtime.interrupt(session.id, PEOPLE));
    expect(s).toMatchObject({ status: "interrupted", statusReason: "userInterrupt", rootBranch: "br1" });
    expect(h.registry.created.map((c) => c.name)).toEqual(["agent.editor-1"]);
    expect(h.registry.list("P1").map((b) => b.id)).toEqual(["main", "br1"]);
    expect(credential(h, session.id)).toBe("live");
    const [call] = store.nodes(session.id).filter((n) => n.kind === "toolCall");
    // The tool finishes unheard: the synthetic result stands, nothing else is written.
    h.tool.release();
    await new Promise((r) => setTimeout(r, 30));
    const results = store.nodes(session.id).filter((n) => n.kind === "toolResult");
    expect(results.map((n) => [n.callId, payload(n).synthetic])).toEqual([[call!.callId, true]]);
    expect(kinds(session.id)).toEqual(["userMessage", "toolCall", "toolResult"]);
    expect(h.runtime.stats().live).toBe(0);
    expect((await failure(h.runtime.prompt(session.id, "again", PEOPLE))).code).toBe("SESSION_INTERRUPTED");
  });

  const edit: FakeScript = {
    name: "edit",
    turns: [[...toolTurn(["call_1", ["item", "trim", "c1", "--end", "-1"]], ["call_2", ["view"]]), { emit: [{ type: "message", role: "assistant", blocks: [{ type: "text", text: "done" }] }] }]],
  };
  const check: FakeScript = {
    name: "check",
    turns: [[...toolTurn(["call_9", ["log"]]), { emit: [{ type: "message", role: "assistant", blocks: [{ type: "text", text: "checked" }] }] }]],
  };

  test.each(["announce", "started", "commit", "result"] as const)("resume_after_crash_never_reruns_tool (crash %s)", async (point) => {
    const tool = new FinTool();
    const first = makeRuntime({ store, control }, FakeAdapter([edit]), { clock, tool });
    let crashed!: () => void;
    const down = new Promise<void>((r) => (crashed = r));
    const crash = () => void first.runtime.abandon().then(crashed);
    const isTrim = (argv: unknown) => (argv as string[])[0] === "item";
    tool.before = async (c) => {
      const argv = (c.input as { argv: string[] }).argv;
      if (point === "announce" && isTrim(argv)) return (crash(), "hang");
      if (point === "started" && isTrim(argv)) {
        // The `fin` process outlives the server and commits after it died.
        setTimeout(() => tool.commit(c.callId, argv), 30);
        return (crash(), "hang");
      }
      if (point === "result" && argv[0] === "view") return (crash(), "hang");
      return "continue";
    };
    tool.afterCommit = point === "commit" ? async () => (crash(), "hang") : null;
    await first.runtime.boot();
    const { session } = await run(first.runtime.start({ projectId: "P1", prompt: "trim c1" }, PEOPLE, META));
    await down;
    await new Promise((r) => setTimeout(r, 60));

    // The restart: a new store and runtime over the same control.db.
    tool.before = null;
    tool.afterCommit = null;
    const before = tool.calls.length;
    store = new AgentStore(control, dir, clock, systemIds(clock));
    const second = harness(FakeAdapter([check]), { tool });
    const rec = await second.runtime.boot();
    expect(rec.interrupted).toEqual([session.id]);
    expect(store.get(session.id)!.statusReason).toBe(SERVER_RESTART);
    await run(second.runtime.resume(session.id, PEOPLE, "continue"));
    await until(idle(session.id), "idle after resume");

    const trim = store.nodes(session.id).find((n) => n.kind === "toolCall" && n.providerId === "call_1")!;
    const results = store.nodes(session.id).filter((n) => n.kind === "toolResult" && n.callId === trim.callId);
    expect(results).toHaveLength(1);
    expect(payload(results[0]!).synthetic === true).toBe(point !== "result");
    // The edit ran once, under its call id, and is never run again after the restart.
    expect(tool.calls.filter((c) => c.callId === trim.callId)).toHaveLength(1);
    expect(tool.calls.slice(before).map((c) => (c.input as { argv: string[] }).argv[0])).toEqual(["log"]);
    expect([...tool.commits.keys()]).toEqual(point === "announce" ? [] : [trim.callId]);
    expect(store.outbox()).toEqual([]);
  });

  test("steer_is_queued_before_next_call", async () => {
    // A provider that takes steers mid-run (the built-in loop appends it before its next call).
    const adapter = FakeAdapter([fixture("tool")]);
    const h = harness(adapter);
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1, "the tool running");
    expect(await run(h.runtime.steer(session.id, "also fix the captions", PEOPLE))).toEqual({ queued: false });
    expect(adapter.log.steers.map((s) => s.text)).toEqual(["also fix the captions"]);
    h.tool.hold = false;
    h.tool.release();
    await until(idle(session.id), "idle");
    expect(kinds(session.id)).toEqual(["userMessage", "toolCall", "userMessage", "toolResult", "toolCall", "toolResult", "assistantMessage", "usage"]);
    expect(payload(store.nodes(session.id)[2]!)).toEqual({ text: "also fix the captions", trigger: "steer" });

    // A provider that cannot: the text is held and becomes the next run.
    const held = FakeAdapter([{ ...fixture("tool"), capabilities: { steer: false } }]);
    const h2 = harness(held);
    await h2.runtime.boot();
    h2.tool.hold = true;
    const { session: s2 } = await run(h2.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    await until(() => h2.tool.waiting().length === 1, "the tool running");
    expect(await run(h2.runtime.steer(s2.id, "and the outro", PEOPLE))).toEqual({ queued: true });
    h2.tool.hold = false;
    h2.tool.release();
    await until(() => store.runs(s2.id).length === 2 && status(s2.id) === "idle", "the steer's run");
    expect(store.runs(s2.id).map((r) => [r.trigger, r.status])).toEqual([
      ["prompt", "completed"],
      ["steer", "completed"],
    ]);
    expect(held.log.steers).toEqual([]);
    expect(held.log.turns.map((t) => t.text)).toEqual(["trim", "and the outro"]);
  });

  test("watch_snapshot_then_live", async () => {
    const h = harness(FakeAdapter([fixture("tool")]));
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    const a = watch(h, session.id);
    await run(h.runtime.prompt(session.id, "trim", PEOPLE));
    await until(() => h.tool.waiting().length === 1, "the first tool running");
    // A watcher joining mid-run: the stored nodes first, then live ones, each once.
    const b = watch(h, session.id);
    const c = watch(h, session.id, 2);
    await until(() => b.nodes().length === 2, "b's snapshot");
    h.tool.hold = false;
    h.tool.release();
    await until(idle(session.id), "idle");
    const seqs = store.nodes(session.id).map((n) => n.seq);
    await until(() => a.nodes().length === seqs.length && b.nodes().length === seqs.length, "all nodes watched");
    expect(a.nodes().map((n) => n.seq)).toEqual(seqs);
    expect(b.nodes().map((n) => n.seq)).toEqual(seqs);
    expect(c.nodes().map((n) => n.seq)).toEqual(seqs.slice(1));
    await run(h.runtime.end(session.id, PEOPLE));
    // An ended session ends its watches.
    await Effect.runPromise(Fiber.join(a.fiber));
    expect(a.items.at(-1)).toEqual({ type: "statusChanged", status: "ended", reason: "userEnded" });
    expect(credential(h, session.id)).toBe("revoked");
    const late = watch(h, session.id);
    await Effect.runPromise(Fiber.join(late.fiber));
    expect(late.nodes()).toHaveLength(seqs.length);
    expect((await failure(h.runtime.watch("01JC0000000000000000000099").pipe(Stream.runCollect))).code).toBe("SESSION_NOT_FOUND");
  });

  test("budget_stop_at_safe_point", async () => {
    const script: FakeScript = {
      name: "spend",
      turns: [
        [
          { emit: [{ type: "toolStarted", providerId: "call_1", tool: "fin", input: { argv: ["item", "trim", "c1"] } }, { type: "usage", input: 5000, output: 10, cacheRead: 0, cacheWrite: 0, reasoning: 0 }] },
          { awaitTool: "call_1" },
          { emit: [{ type: "message", role: "assistant", blocks: [{ type: "text", text: "never seen" }] }] },
        ],
      ],
    };
    const h = harness(FakeAdapter([script]));
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1 && kinds(session.id).includes("usage"), "usage during the tool call");
    // The stop waits for the running call's result.
    expect(status(session.id)).toBe("running");
    h.tool.release();
    await until(() => status(session.id) === "interrupted", "the stop");
    expect(store.get(session.id)!.statusReason).toBe("BUDGET_EXCEEDED");
    const nodes = store.nodes(session.id);
    const result = nodes.find((n) => n.kind === "toolResult")!;
    expect(payload(result)).toMatchObject({ ok: true, output: { txId: expect.any(String) } });
    expect(nodes.map((n) => n.kind)).toEqual(["userMessage", "toolCall", "usage", "notice", "toolResult", "error"]);
    expect(payload(nodes.at(-1)!)).toMatchObject({ code: "BUDGET_EXCEEDED", details: { kind: "tokens", used: 5010, limit: 1000 } });
    expect([...h.tool.commits.keys()]).toHaveLength(1);
    const [approval] = store.approvals(session.id);
    expect(approval).toMatchObject({ kind: "budget", status: "open", subject: { kind: "tokens", used: 5010, limit: 1000 } });
    expect((await failure(h.runtime.prompt(session.id, "more", PEOPLE))).code).toBe("SESSION_INTERRUPTED");
    // `allow --add` raises the limit and resumes.
    const answered = await run(h.runtime.approve(approval!.id, "allow", { tokens: 100_000 }, PEOPLE));
    expect(answered.session.status).toBe("idle");
    expect(answered.approval).toMatchObject({ status: "answered", decision: "allow", by: "local:local", consumedAt: expect.any(Number) });
    expect(store.ledger().read(session.id, "tokens").limit).toBe(101_000);
    expect(credential(h, session.id)).toBe("live");
    // Opened, then answered (Task 10 records the decision).
    expect(h.journal.entries.filter((e) => e.kind === "approval").map((e) => e.id)).toEqual([approval!.id, approval!.id]);
    expect((await failure(h.runtime.approve("01JC0000000000000000000099", "allow", {}, PEOPLE))).code).toBe("APPROVAL_NOT_FOUND");
  });

  test("a run over budget does not start", async () => {
    const h = harness(FakeAdapter([fixture("simple")]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => status(session.id) !== "running", "the run's end");
    // 2140 tokens used of 1000: the stop came after the turn's last event.
    expect(status(session.id)).toBe("interrupted");
    await run(h.runtime.resume(session.id, PEOPLE));
    const e = await failure(h.runtime.prompt(session.id, "more", PEOPLE));
    expect(e).toMatchObject({ code: "BUDGET_EXCEEDED", details: { kind: "tokens", limit: 1000 } });
    expect(store.runs(session.id)).toHaveLength(1);
  });

  test("budget_facts_are_in_a_message_not_the_system_prompt", async () => {
    const adapter = FakeAdapter([fixture("simple")]);
    const h = harness(adapter);
    await h.runtime.boot();
    const a = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 1000 } }, PEOPLE, META));
    const b = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 5000, moneyMicros: 2_000_000 } }, PEOPLE, META));
    await until(() => store.runs(a.session.id)[0]!.status !== "running" && store.runs(b.session.id)[0]!.status !== "running", "both runs");
    const [sa, sb] = adapter.log.opened.map((o) => o.spec);
    expect(sa!.system).toBe(sb!.system);
    expect(sa!.system).not.toMatch(/token|USD|budget/i);
    const [ta, tb] = adapter.log.turns;
    expect(ta!.facts).toContain("1000 of 1000 tokens");
    expect(tb!.facts).toContain("5000 of 5000 tokens");
    expect(tb!.facts).toContain("2.00 of 2.00 USD");
    expect(ta!.facts).toContain(`branch ${a.session.rootBranch}, mode draft`);
    expect(ta!.text).toBe("go");
  });

  test("turn failures: context end keeps the session idle, others interrupt it", async () => {
    const fail = (code: string): FakeScript => ({ turns: [[{ emit: [{ type: "turnFailed", error: { code, message: code, details: {} } }] }]] });
    const h = harness(FakeAdapter([fail("MODEL_CONTEXT_EXCEEDED")]));
    await h.runtime.boot();
    const a = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(idle(a.session.id), "idle");
    expect(store.runs(a.session.id)).toMatchObject([{ status: "completed", stopReason: "MODEL_CONTEXT_EXCEEDED" }]);
    const h2 = harness(FakeAdapter([fail("PROVIDER_OVERLOADED")]));
    await h2.runtime.boot();
    const b = await run(h2.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => status(b.session.id) === "interrupted", "interrupted");
    expect(store.get(b.session.id)!.statusReason).toBe("PROVIDER_OVERLOADED");
    expect(kinds(b.session.id)).toEqual(["userMessage", "error"]);
  });

  test("a runtime without provider permissions declines a provider request by policy (7A's answer)", async () => {
    const adapter = FakeAdapter([fixture("approval")]);
    const h = harness(adapter);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "merge" }, PEOPLE, META));
    await until(idle(session.id), "idle");
    expect(payload(store.nodes(session.id)[1]!)).toMatchObject({ requestId: "req_1", decision: "deny", by: "policy" });
    expect(adapter.log.answers.map((a) => a.answer)).toEqual([{ kind: "approval", allow: false, message: expect.any(String) }]);
  });

  test("boot recovers, sweeps orphans and drains the outbox", async () => {
    const s = await store.createSession({
      id: systemIds(clock).ulid(),
      projectId: "P1",
      harness: "fake",
      provider: "openrouter",
      model: "m",
      weights: "closed",
      role: "editor",
      mode: "draft",
      rootBranch: "br1",
      base: { branch: "main", seq: 1 },
      budget: {},
      createdBy: { principal: { kind: "local", id: "local" } },
    });
    store.apply(s.id, { kind: "ready" });
    store.startRun(s.id);
    const orphan = systemIds(clock).ulid();
    mkdirSync(join(dir, "agents", orphan), { recursive: true });
    clock.advance(1000);
    const h = harness(FakeAdapter([fixture("simple")]));
    expect((await failure(h.runtime.prompt(s.id, "go", PEOPLE))).code).toBe("SERVER_UNAVAILABLE");
    const r = await h.runtime.boot();
    expect(r).toMatchObject({ interrupted: [s.id], closedCalls: 0, orphans: 1 });
    expect(existsSync(join(dir, "agents", orphan))).toBe(false);
    expect(store.outbox()).toEqual([]);
    expect(h.journal.entries.map((e) => [e.kind, e.phase])).toEqual([
      ["session", "started"],
      ["session", "interrupted"],
    ]);
  });

  test("an outbox effect that fails stays queued and holds its session's later ones", async () => {
    const h = harness(FakeAdapter([fixture("simple")]));
    await h.runtime.boot();
    const a = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    const b = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    h.journal.failSessions = true;
    await run(h.runtime.end(a.session.id, PEOPLE));
    // The credential effect went first (Task 7's handler), the journal entry stays owed.
    expect(credential(h, a.session.id)).toBe("revoked");
    expect(store.outbox(a.session.id).map((o) => o.effect.kind)).toEqual(["journal"]);
    h.journal.failSessions = false;
    await run(h.runtime.delete(a.session.id, PEOPLE));
    expect(store.outbox()).toEqual([]);
    expect(store.get(a.session.id)).toBeUndefined();
    expect(store.get(b.session.id)!.status).toBe("idle");
    expect(h.journal.entries.filter((e) => e.kind === "session" && e.sessionId === a.session.id).map((e) => e.phase)).toEqual(["started", "ended"]);
  });

  test("resume refreshes the credential first: an ended one refuses it", async () => {
    const h = harness(FakeAdapter([fixture("simple")]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    await run(h.runtime.interrupt(session.id, PEOPLE));
    await Effect.runPromise(h.core.credentials.revokeForSession(session.id));
    expect((await failure(h.runtime.resume(session.id, PEOPLE))).code).toBe("CREDENTIAL_REVOKED");
    expect(status(session.id)).toBe("interrupted");
  });

  test("start: plan mode has no branch; a base that does not exist; unknown harness", async () => {
    const h = harness(FakeAdapter([fixture("simple")]));
    await h.runtime.boot();
    const p = await run(h.runtime.start({ projectId: "P1", mode: "plan" }, PEOPLE, META));
    expect(p.session.rootBranch).toBeUndefined();
    expect(p.session.base).toEqual({ branch: "main", seq: 3 });
    expect(h.registry.created).toEqual([]);
    expect((await failure(h.runtime.start({ projectId: "P1", base: "nope" }, PEOPLE, META))).code).toBe("BRANCH_NOT_FOUND");
    expect(store.list().sessions.map((s) => s.id)).toEqual([p.session.id]);
    expect((await failure(h.runtime.start({ projectId: "P1", harness: "codex" }, PEOPLE, META))).code).toBe("HARNESS_UNKNOWN");
  });

  test("delete removes a live session after ending it", async () => {
    const h = harness(FakeAdapter([fixture("tool")]));
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "trim" }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1, "the tool running");
    store.recordTx({ txId: "T-open", sessionId: session.id, callId: null, attribution: "none", at: 0 });
    await run(h.runtime.delete(session.id, PEOPLE));
    expect(store.get(session.id)).toBeUndefined();
    // Its unmerged transaction is abandoned (Task 11), and the row outlives it.
    expect(store.outcomes.of("T-open")).toMatchObject([{ verdict: "abandoned", via: "archive", by: "user" }]);
    expect(store.sessionOfTx(["T-open"]).get("T-open")?.outcome).toBe("abandoned");
    expect(existsSync(join(dir, "agents", session.id))).toBe(false);
    expect(credential(h, session.id)).toBe("revoked");
    expect(h.runtime.stats().live).toBe(0);
    h.tool.release();
    expect((await failure(h.runtime.prompt(session.id, "x", PEOPLE))).code).toBe("SESSION_NOT_FOUND");
  });

  test("call_gate_reserves_before_a_model_call_and_stops_at_the_cap", async () => {
    const usage = { type: "usage", input: 10, output: 10, cacheRead: 0, cacheWrite: 0, reasoning: 0, costMicros: 400 };
    const adapter = FakeAdapter([{ turns: [[...toolTurn(["call_1", ["view"]]), { emit: [usage] }], toolTurn(["call_2", ["view"]])] }]);
    const h = harness(adapter, { pricing: { ...TEST_PRICING, projected: () => 1000 } });
    await h.runtime.boot();
    h.tool.hold = true;
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { modelCallsPerRun: 1, moneyMicros: 1200 } }, PEOPLE, META));
    await until(() => h.tool.waiting().length === 1, "the tool running");
    const gate = adapter.log.opened[0]!.spec.gate!;
    const projected = { input: 10, output: 10, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
    expect(gate.beforeCall(projected)).toEqual({ ok: true });
    expect(store.ledger().read(session.id, "money").reserved).toBe(1000);
    // The call's usage settles its reservation: 400 spent, nothing held.
    h.tool.release();
    await until(() => store.ledger().read(session.id, "money").used === 400, "the usage charged");
    expect(store.ledger().read(session.id, "money").reserved).toBe(0);
    // The per-run cap refuses the next call but never stops the session.
    expect(gate.beforeCall(projected)).toMatchObject({ ok: false, error: { code: "BUDGET_EXCEEDED", details: { kind: "modelCalls" } } });
    await until(idle(session.id), "idle");
    // A projection over the money left stops the session, after the call in flight.
    await run(h.runtime.prompt(session.id, "more", PEOPLE));
    await until(() => h.tool.waiting().length === 1, "the second tool running");
    expect(gate.beforeCall(projected)).toMatchObject({ ok: false, error: { code: "BUDGET_EXCEEDED", details: { kind: "money" } } });
    await new Promise((r) => setTimeout(r, 20));
    expect(status(session.id)).toBe("running");
    h.tool.release();
    await until(() => status(session.id) === "interrupted", "the stop");
    expect(store.approvals(session.id)).toMatchObject([{ kind: "budget", subject: { kind: "money" } }]);
    gate.release();
    expect(store.ledger().read(session.id, "money").reserved).toBe(0);
  });

  test("the wall clock stops a silent run at the next check", async () => {
    const h = harness(FakeAdapter([{ turns: [[{ awaitAnswer: "never" }]] }]));
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "think", budget: { wallMs: 60_000 } }, PEOPLE, META));
    h.runtime.checkBudgets();
    await new Promise((r) => setTimeout(r, 20));
    expect(status(session.id)).toBe("running");
    clock.advance(50_000);
    h.runtime.checkBudgets();
    await until(() => kinds(session.id).includes("notice"), "the 80 % notice");
    clock.advance(11_000);
    h.runtime.checkBudgets();
    await until(() => status(session.id) === "interrupted", "the stop");
    expect(store.approvals(session.id)).toMatchObject([{ kind: "budget", subject: { kind: "wallMs", limit: 60_000 } }]);
    expect(store.runs(session.id)).toMatchObject([{ status: "interrupted", stopReason: "BUDGET_EXCEEDED" }]);
  });

  test("a SIGKILLed server: the commit landed once, and resume does not run it again", async () => {
    const src = (p: string) => JSON.stringify(pathToFileURL(join(import.meta.dirname, "..", "src", p)).href);
    const support = JSON.stringify(pathToFileURL(join(import.meta.dirname, "support", "runtime-doubles.ts")).href);
    const commits = join(dir, "commits");
    mkdirSync(commits);
    const script = join(dir, "crash.mjs");
    writeFileSync(
      script,
      `const { openControlStore } = await import(${src("control/db.ts")});
       const { AgentStore } = await import(${src("agent/store.ts")});
       const { FakeAdapter } = await import(${src("agent/fake.ts")});
       const { systemIds, systemClock } = await import(${src("agent/clock.ts")});
       const { makeRuntime, PEOPLE, META, FinTool } = await import(${support});
       const { Effect } = await import(${JSON.stringify(import.meta.resolve("effect"))});
       const { writeFileSync } = await import("node:fs");
       const [data, commits, edit] = process.argv.slice(2);
       const control = openControlStore(data, { serverId: "sid", checkpoint: false });
       const store = new AgentStore(control, data, systemClock, systemIds());
       const tool = new FinTool();
       tool.commit = (key) => writeFileSync(commits + "/" + key, "1", { flag: "wx" });
       tool.afterCommit = async () => process.kill(process.pid, "SIGKILL");
       const h = makeRuntime({ store, control }, FakeAdapter([JSON.parse(edit)]), { tool });
       await h.runtime.boot();
       await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "trim c1" }, PEOPLE, META));
       await new Promise((r) => setTimeout(r, 10_000));`,
    );
    await control.close();
    const crashDir = join(dir, "crash");
    mkdirSync(crashDir);
    const child = spawnSync(process.execPath, [script, crashDir, commits, JSON.stringify(edit)], { encoding: "utf8", env: process.env, cwd: import.meta.dirname, timeout: 30_000 });
    if (process.platform !== "win32") expect(child.signal, child.stderr).toBe("SIGKILL");
    const landed = readdirSync(commits);
    expect(landed).toHaveLength(1);
    control = openControlStore(crashDir, { serverId: "sid", checkpoint: false });
    store = new AgentStore(control, crashDir, clock, systemIds(clock));
    const tool = new FinTool();
    const h = harness(FakeAdapter([check]), { tool });
    const rec = await h.runtime.boot();
    expect(rec).toMatchObject({ interrupted: [expect.any(String)], closedCalls: 1 });
    const id = rec.interrupted[0]!;
    const trim = store.nodes(id).find((n) => n.kind === "toolCall")!;
    expect(landed).toEqual([trim.callId]);
    await run(h.runtime.resume(id, PEOPLE, "continue"));
    await until(idle(id), "idle after resume");
    expect(tool.calls.map((c) => (c.input as { argv: string[] }).argv[0])).toEqual(["log"]);
    expect(readdirSync(commits)).toEqual([trim.callId]);
    expect(store.nodes(id).filter((n) => n.kind === "toolResult" && n.callId === trim.callId).map((n) => payload(n).code)).toEqual(["SESSION_INTERRUPTED"]);
  });
});
