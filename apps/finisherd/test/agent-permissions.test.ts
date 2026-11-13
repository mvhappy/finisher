// Provider permission requests and the built-in loop's waits (sub-project 7B Task 5, Decisions
// 11 and 13; spec §7.3, §7.4): `decideProviderRequest` allows what the sandbox contains, denies
// what the mode forbids and routes a rule's `ask` (by tool pattern) to a `provider` approval
// whose answer goes back through `respond`, each decision an `approval` node with `by`; a
// built-in `fin` call that meets `APPROVAL_PENDING` waits on the card (`waitingApproval`) and
// the model gets the outcome as the tool result.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, Node, Principal, Rule, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AgentApprovals, argsHash, subjectOf } from "../src/agent/approvals.ts";
import { pendingApproval, waitingOnApprovals } from "../src/agent/builtin/approval-wait.ts";
import { type ModelClient, type ModelResult, type ModelStreamEvent } from "../src/agent/builtin/compat.ts";
import { BuiltinAdapter } from "../src/agent/builtin/loop.ts";
import type { ModelRequest, ToolCall as ModelToolCall } from "../src/agent/builtin/request.ts";
import { Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { insideWorkspace, providerPermissions, TOOL_TEXT_MAX, toolNames } from "../src/agent/permissions.ts";
import { ProviderRequests } from "../src/agent/provider-requests.ts";
import { remember } from "../src/agent/remember.ts";
import type { RuleSource } from "../src/agent/rules.ts";
import { cataloguePricing, type ToolCall, type ToolExecutor, type ToolOutcome } from "../src/agent/runtime-deps.ts";
import { AgentStore } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

const KEY = Buffer.alloc(32, 3);
const PERSON: Principal = { kind: "console", id: "owner", scopes: [], projects: "*", branches: "*", human: true };
const rule = (id: string, effect: Rule["effect"], command: string): Rule => ({ id, effect, command, layer: "local", source: "test", createdAt: 0 });
const source = (rules: Rule[]): RuleSource => ({ layers: () => ({ rules, projects: new Map() }) });
const session = (mode: Session["mode"]) => ({ id: "S1", projectId: "P1", mode, role: "editor", provider: "openrouter" }) as unknown as Session;

describe("decideProviderRequest", () => {
  const decide = (rules: Rule[], mode: Session["mode"], tool: string, input: unknown) =>
    Effect.runPromise(providerPermissions({ rules: source(rules), dirOf: () => "/proj" }).decideProviderRequest(session(mode), { tool, input }));

  test("allows what the sandbox contains: workspace writes, reads, shell commands", async () => {
    for (const mode of ["plan", "draft", "auto"] as const) {
      expect(await decide([], mode, "Write", { file_path: "/proj/workspace/S1/notes.md" }), mode).toBe("allow");
      expect(await decide([], mode, "Edit", { file_path: "drafts/cut.txt" }), mode).toBe("allow");
      expect(await decide([], mode, "Read", { file_path: "/etc/hosts" }), mode).toBe("allow");
      expect(await decide([], mode, "Bash", { command: "ls -la" }), mode).toBe("allow");
    }
  });

  test("denies what the mode forbids: a write outside workspace/<session>/ in plan", async () => {
    expect(await decide([], "plan", "Write", { file_path: "/proj/finisher.rules.json" })).toBe("deny");
    expect(await decide([], "plan", "Edit", { file_path: "../../finisher.project.json" })).toBe("deny");
    expect(await decide([], "plan", "Write", { file_path: "/proj/workspace/S2/x" })).toBe("deny");
    expect(await decide([], "plan", "apply_patch", { patch: "…" })).toBe("deny");
    // In draft the sandbox (7C) is the bound, not this check.
    expect(await decide([], "draft", "Write", { file_path: "/proj/x.txt" })).toBe("allow");
    // The mode's deny comes before a rule's ask.
    expect(await decide([rule("a", "ask", "tool:Write")], "plan", "Write", { file_path: "/proj/x" })).toBe("deny");
  });

  test("asks what a rule marks by tool pattern; a deny rule denies", async () => {
    const rules = [rule("fetch", "ask", "tool:WebFetch"), rule("curl", "ask", "tool:Bash(curl *)"), rule("rm", "deny", "tool:Bash(rm *)")];
    expect(await decide(rules, "draft", "WebFetch", { url: "https://example.com" })).toEqual({ ask: "fetch" });
    expect(await decide(rules, "draft", "Bash", { command: "curl https://example.com" })).toEqual({ ask: "curl" });
    expect(await decide(rules, "draft", "Bash", { command: "ls" })).toBe("allow");
    expect(await decide(rules, "draft", "Bash", { command: "rm -rf /" })).toBe("deny");
    // A broken layer's deny-everything holds for tools too.
    expect(await decide([rule("local.unreadable", "deny", "**")], "auto", "Read", { file_path: "x" })).toBe("deny");
  });

  test("a long input is matched in full: padding cannot push a command past a *…* rule", async () => {
    const rules = [rule("rm", "deny", "tool:Bash(*rm -rf*)"), rule("curl", "ask", "tool:Bash(*curl*)")];
    expect(await decide(rules, "auto", "Bash", { command: `${" ".repeat(5000)}rm -rf /` })).toBe("deny");
    expect(await decide(rules, "auto", "Bash", { command: `${"x".repeat(100_000)} curl https://e.x` })).toEqual({ ask: "curl" });
  });

  test("an input past TOOL_TEXT_MAX is never allowed: denied when a deny rule names the tool, else asked", async () => {
    const huge = { command: `${" ".repeat(TOOL_TEXT_MAX)}ls` };
    expect(await decide([rule("rm", "deny", "tool:Bash(*rm -rf*)")], "auto", "Bash", huge)).toBe("deny");
    expect(await decide([rule("any", "deny", "tool:B*(x)")], "auto", "Bash", huge)).toBe("deny");
    expect(await decide([], "auto", "Bash", huge)).toEqual({ ask: "tool.oversized" });
    expect(await decide([rule("c", "ask", "tool:Bash(*curl*)")], "auto", "Bash", huge)).toEqual({ ask: "c" });
    // A deny rule of another tool does not deny it.
    expect(await decide([rule("w", "deny", "tool:WebFetch(*)")], "auto", "Bash", huge)).toEqual({ ask: "tool.oversized" });
  });

  test("a provider card's command is remembered only when a rule can hold it exactly", () => {
    const deps = { ruleStore: undefined as never, sessions: { get: () => session("draft") }, dirOf: () => "/proj", audit: undefined as never, clock: manualClock(0) };
    for (const command of ["tool:Bash(ls | wc -l)", "tool:Bash(ls *.txt)", "tool:Bash(echo \"x\")"]) {
      const a = { id: "A1", sessionId: "S1", kind: "provider", status: "open", subject: { command }, argsHash: "h", requestedAt: 0, expiresAt: 1 } as const;
      expect(() => remember(deps, a, "deny", "projectLocal", PERSON), command).toThrow(expect.objectContaining({ code: "INVALID_REQUEST", details: expect.objectContaining({ reason: "notExact" }) }));
    }
  });

  test("names and paths", () => {
    expect(toolNames("WebFetch", { url: "https://a.b" })).toEqual(["tool:WebFetch", "tool:WebFetch(https://a.b)"]);
    expect(toolNames("Thing", null)).toEqual(["tool:Thing"]);
    expect(toolNames("Thing", { b: 1, a: 2 })).toEqual(["tool:Thing", 'tool:Thing({"a":2,"b":1})']);
    expect(insideWorkspace("/w/S1", "a/b")).toBe(true);
    expect(insideWorkspace("/w/S1", "/w/S1")).toBe(true);
    expect(insideWorkspace("/w/S1", "/w/S10/x")).toBe(false);
    expect(insideWorkspace("/w/S1", "a/../../x")).toBe(false);
  });
});

// --- In the runtime -----------------------------------------------------------------------

let dir: string;
let control: ControlStore;
let clock: ReturnType<typeof manualClock>;
let store: AgentStore;
const cleanups: (() => Promise<void>)[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-permissions-"));
  clock = manualClock(Date.now());
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

/** The runtime with provider permissions and real approvals, as `wiring.ts` builds them. */
function routed(adapter: Parameters<typeof makeRuntime>[1], rules: Rule[], opts: Parameters<typeof makeRuntime>[2] = {}, schedule?: Schedule) {
  let approvals: AgentApprovals | undefined;
  const permissions = providerPermissions({ rules: source(rules), dirOf: () => "/" });
  const requests = new ProviderRequests({ permissions, approvals: () => approvals, key: KEY, consume: (id) => store.consumeApproval(id), ...(schedule === undefined ? {} : { schedule }) });
  const h = makeRuntime({ store, control }, adapter, { clock, requests, ...opts });
  approvals = new AgentApprovals({
    control,
    store,
    clock,
    opened: (s, a, node) => h.runtime.approvalOpened(s, a, node),
    approve: (id, decision, by) => h.runtime.approve(id, decision, {}, by),
  });
  cleanups.push(() => h.runtime.close());
  return { ...h, approvals, requests };
}

type Schedule = (ms: number, f: () => void) => () => void;
/** Timers a test fires by hand (with the injected clock). */
function handTimers() {
  const timers: Array<{ ms: number; f: () => void; cleared: boolean }> = [];
  const schedule: Schedule = (ms, f) => {
    // A timer that fired is spent, as a cleared one.
    const t = { ms, f: () => ((t.cleared = true), f()), cleared: false };
    timers.push(t);
    return () => void (t.cleared = true);
  };
  return { timers, schedule, live: () => timers.filter((t) => !t.cleared) };
}

const approvalNodes = (sessionId: string) => store.nodes(sessionId).filter((n: Node) => n.kind === "approval").map((n) => n.payload as Record<string, unknown>);
const request = (id: string, tool: string, input: unknown) => [{ emit: [{ type: "request", id, kind: "approval", tool, input }] }, { awaitAnswer: id }];

describe("provider requests in the runtime", () => {
  test("provider_requests_routed: workspace write allowed, write outside in plan denied, tool:WebFetch asks and the answer reaches respond", async () => {
    const script: FakeScript = {
      turns: [[
        ...request("req_1", "Write", { file_path: "notes.md", content: "x" }),
        ...request("req_2", "Write", { file_path: "/etc/passwd", content: "x" }),
        ...request("req_3", "WebFetch", { url: "https://example.com" }),
        { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] },
      ]],
    };
    const adapter = FakeAdapter([script]);
    const h = routed(adapter, [rule("fetch", "ask", "tool:WebFetch")]);
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "go", mode: "plan" }, PEOPLE, META));
    await until(() => store.approvals(s.id).length === 1, "the provider card");
    const card = store.approvals(s.id)[0]!;
    expect(card).toMatchObject({ kind: "provider", status: "open", subject: { command: "tool:WebFetch", rpc: "provider.request", args: { url: "https://example.com" }, rule: "fetch", layer: "local" } });
    expect(store.get(s.id)!.status).toBe("waitingApproval");
    // The request waits: no answer for req_3 yet.
    expect(adapter.log.answers.map((a) => [a.id, a.answer])).toEqual([
      ["req_1", { kind: "approval", allow: true }],
      ["req_2", { kind: "approval", allow: false, message: expect.stringMatching(/plan mode/) }],
    ]);
    expect(h.requests.waiting).toBe(1);
    await Effect.runPromise(h.approvals.answer(card.id, "allow", PERSON));
    await until(() => store.get(s.id)!.status === "idle", "idle");
    expect(adapter.log.answers.map((a) => [a.id, a.answer])).toContainEqual(["req_3", { kind: "approval", allow: true }]);
    expect(store.approval(card.id)).toMatchObject({ status: "answered", decision: "allow", by: "console:owner" });
    expect(store.approval(card.id)!.consumedAt).toBeTypeOf("number");
    expect(approvalNodes(s.id)).toEqual([
      expect.objectContaining({ requestId: "req_1", kind: "provider", tool: "Write", decision: "allow", by: "policy", reason: "sandbox" }),
      expect.objectContaining({ requestId: "req_2", kind: "provider", tool: "Write", decision: "deny", by: "policy", reason: "mode" }),
      expect.objectContaining({ approvalId: card.id, kind: "provider", status: "open", command: "tool:WebFetch", rule: "fetch" }),
      expect.objectContaining({ approvalId: card.id, requestId: "req_3", kind: "provider", decision: "allow", by: "user" }),
    ]);
  });

  test("a denied or expired provider card is a deny the provider hears; by user, or by policy on expiry", async () => {
    const script: FakeScript = { turns: [[...request("r1", "WebFetch", { url: "https://a.example" }), ...request("r2", "WebFetch", { url: "https://b.example" }), { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] }]] };
    const adapter = FakeAdapter([script]);
    const h = routed(adapter, [rule("fetch", "ask", "tool:WebFetch")]);
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.approvals(s.id).length === 1, "the first card");
    await Effect.runPromise(h.approvals.answer(store.approvals(s.id)[0]!.id, "deny", PERSON));
    await until(() => store.approvals(s.id).length === 2, "the second card");
    const second = store.approvals(s.id).find((a) => a.status === "open")!;
    clock.advance(second.expiresAt - clock.now());
    await h.runtime.sweep();
    await until(() => store.get(s.id)!.status === "idle", "idle");
    expect(adapter.log.answers.map((a) => [a.id, a.answer])).toEqual([
      ["r1", { kind: "approval", allow: false, message: expect.any(String) }],
      ["r2", { kind: "approval", allow: false, message: expect.any(String) }],
    ]);
    const settled = approvalNodes(s.id).filter((n) => n.requestId !== undefined);
    expect(settled).toEqual([
      expect.objectContaining({ requestId: "r1", decision: "deny", by: "user" }),
      expect.objectContaining({ requestId: "r2", decision: "deny", by: "policy", reason: "expired" }),
    ]);
  });
});

describe("provider card expiry", () => {
  test("provider_card_settles_at_expiry: a timer at expiresAt settles the request (deny by policy), no sweep needed; an answer clears it", async () => {
    const script: FakeScript = { turns: [[...request("r1", "WebFetch", { url: "https://a.example" }), ...request("r2", "WebFetch", { url: "https://b.example" }), { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] }]] };
    const adapter = FakeAdapter([script]);
    const t = handTimers();
    const h = routed(adapter, [rule("fetch", "ask", "tool:WebFetch")], {}, t.schedule);
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.approvals(s.id).length === 1, "the first card");
    const first = store.approvals(s.id)[0]!;
    expect(t.live()).toHaveLength(1);
    expect(t.live()[0]!.ms).toBe(first.expiresAt - clock.now());
    // Answered: its timer is cleared.
    await Effect.runPromise(h.approvals.answer(first.id, "allow", PERSON));
    await until(() => store.approvals(s.id).length === 2, "the second card");
    const second = store.approvals(s.id).find((a) => a.status === "open")!;
    expect(t.live()).toHaveLength(1);
    // Fired early by real time (the injected clock not there yet): it waits on.
    const early = t.live()[0]!;
    clock.advance(10 * 60_000);
    early.f();
    expect(store.approval(second.id)!.status).toBe("open");
    expect(t.live().map((x) => x.ms)).toEqual([second.expiresAt - clock.now()]);
    // At expiresAt: expired and settled, with no hourly sweep.
    clock.advance(second.expiresAt - clock.now());
    t.live()[0]!.f();
    await until(() => store.get(s.id)!.status === "idle", "idle");
    expect(store.approval(second.id)!.status).toBe("expired");
    expect(adapter.log.answers.map((a) => [a.id, a.answer])).toEqual([
      ["r1", { kind: "approval", allow: true }],
      ["r2", { kind: "approval", allow: false, message: expect.any(String) }],
    ]);
    expect(approvalNodes(s.id).filter((n) => n.requestId === "r2" && n.approvalId !== undefined)).toEqual([expect.objectContaining({ decision: "deny", by: "policy", reason: "expired" })]);
    expect(h.requests.waiting).toBe(0);
    expect(t.live()).toHaveLength(0);
  });

  test("boot expires approvals already overdue (a restart does not wait for the hourly sweep)", async () => {
    const adapter = FakeAdapter([{ turns: [[...request("r1", "WebFetch", { url: "https://a.example" })]] }]);
    const h = routed(adapter, [rule("fetch", "ask", "tool:WebFetch")], { timers: false });
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.approvals(s.id).length === 1, "the card");
    const card = store.approvals(s.id)[0]!;
    await h.runtime.abandon();
    clock.advance(card.expiresAt - clock.now());
    const again = makeRuntime({ store, control }, FakeAdapter([{ turns: [] }]), { clock, timers: false });
    cleanups.push(() => again.runtime.close());
    await again.runtime.boot();
    expect(store.approval(card.id)!.status).toBe("expired");
  });
});

// --- The built-in loop waits on the card ----------------------------------------------------

const CATALOGUE = Catalogue.load(fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url)), {
  extra: [{ id: "acme-closed/big", window: 8192, price: { input: 1_000_000, output: 2_000_000, cacheRead: 100_000, cacheWrite: 1_250_000 } }],
});

function result(text: string, calls: Array<[string, string, unknown]>): ModelResult {
  const toolCalls: ModelToolCall[] = calls.map(([id, name, input]) => ({ id, name, arguments: JSON.stringify(input) }));
  return {
    text,
    toolCalls,
    reasoningText: "",
    reasoning: [],
    stopReason: toolCalls.length > 0 ? "toolUse" : "endTurn",
    usage: { ...ZERO_USAGE, input: 10, output: 5 },
    usageEstimated: false,
    capture: { request: "{}", response: [] },
    bodyHash: "blake3:00",
    latencyMs: 1,
  };
}

function scripted(replies: ModelResult[]): ModelClient & { requests: ModelRequest[] } {
  const requests: ModelRequest[] = [];
  return {
    preset: "openrouter",
    warnings: [],
    requests,
    wireBody: (req) => req as unknown as Record<string, unknown>,
    async *stream(req): AsyncIterable<ModelStreamEvent> {
      requests.push(structuredClone(req));
      yield { type: "done", result: replies[Math.min(requests.length - 1, replies.length - 1)]! };
    },
  };
}

/** A `fin` that meets a rule's card on its first run (as the chain answers it) and commits on the next. */
class PendingFin implements ToolExecutor {
  readonly calls: ToolCall[] = [];
  approvals!: AgentApprovals;
  async run(call: ToolCall): Promise<ToolOutcome> {
    this.calls.push(call);
    if (this.calls.length > 1) return { ok: true, output: { exit: 0, stdout: { txId: call.callId } } };
    const s = store.get(call.sessionId)!;
    const subject = subjectOf({ command: "checkpoint", tag: "history.checkpoint", payload: { projectId: "P1", branch: "br1", name: "c1" }, rule: "r1", layer: "local" });
    const asked = await Effect.runPromise(this.approvals.ask(s, "rule", subject, argsHash(KEY, subject)));
    const error = { code: "APPROVAL_PENDING", message: "waits", details: { approval: asked!.id, command: "checkpoint" } };
    return { ok: false, output: { exit: 8, stdout: { error } } };
  }
}

describe("the built-in loop", () => {
  async function loopRun(decision: "allow" | "deny") {
    const client = scripted([result("", [["c1", "fin", { argv: ["checkpoint", "c1"] }]]), result("Done.", [])]);
    const fin = new PendingFin();
    let approvals: AgentApprovals | undefined;
    const tool = waitingOnApprovals(fin, () => approvals, 40);
    const h = makeRuntime({ store, control }, BuiltinAdapter({ catalogue: CATALOGUE, finPath: "/x", client: () => client }), { clock, tools: { builtin: tool }, pricing: cataloguePricing(CATALOGUE) });
    approvals = new AgentApprovals({ control, store, clock, opened: (s, a, node) => h.runtime.approvalOpened(s, a, node), approve: (id, d, by) => h.runtime.approve(id, d, {}, by) });
    fin.approvals = approvals;
    cleanups.push(() => h.runtime.close());
    await h.runtime.boot();
    const { session: s } = await Effect.runPromise(h.runtime.start({ projectId: "P1", harness: "builtin", model: "acme-closed/big", prompt: "checkpoint it" }, PEOPLE, META));
    await until(() => store.approvals(s.id).length === 1, "the card");
    // The loop waits on it: the session shows waitingApproval and no tool result yet.
    expect(store.get(s.id)!.status).toBe("waitingApproval");
    await new Promise((r) => setTimeout(r, 120));
    expect(store.nodes(s.id).some((n) => n.kind === "toolResult")).toBe(false);
    expect(fin.calls).toHaveLength(1);
    await Effect.runPromise(approvals.answer(store.approvals(s.id)[0]!.id, decision, PERSON));
    await until(() => store.get(s.id)!.status === "idle" && store.nodes(s.id).some((n) => n.kind === "assistantMessage"), "idle");
    const resultNode = store.nodes(s.id).find((n) => n.kind === "toolResult")!;
    return { s, fin, client, result: resultNode.payload as { ok: boolean; output: Record<string, unknown> } };
  }

  test("builtin_tool_waits_on_pending: allowed, the same call runs again (same FIN_CALL) and its result is the tool result", async () => {
    const { fin, result: r, client } = await loopRun("allow");
    expect(fin.calls).toHaveLength(2);
    expect(fin.calls[1]!.callId).toBe(fin.calls[0]!.callId);
    expect(r).toMatchObject({ ok: true, output: { exit: 0, stdout: { txId: fin.calls[0]!.callId } } });
    // The model saw the outcome, not the pending card.
    expect(JSON.stringify(client.requests[1]!.messages)).not.toContain("APPROVAL_PENDING");
  });

  test("builtin_tool_waits_on_pending: denied, the tool result is APPROVAL_DENIED (exit 7)", async () => {
    const { fin, result: r, client } = await loopRun("deny");
    expect(fin.calls).toHaveLength(1);
    expect(r).toMatchObject({ ok: false, output: { exit: 7, error: { code: "APPROVAL_DENIED", details: { decision: "deny" } } } });
    expect(JSON.stringify(client.requests[1]!.messages)).toContain("APPROVAL_DENIED");
  });

  test("pendingApproval reads fin's answer and nothing else", () => {
    const pending = { ok: false, output: { exit: 8, stdout: { error: { code: "APPROVAL_PENDING", details: { approval: "A1" } } } } };
    expect(pendingApproval(pending)).toBe("A1");
    expect(pendingApproval({ ok: false, output: { exit: 7, stdout: { error: { code: "RULE_DENIED", details: {} } } } })).toBeUndefined();
    expect(pendingApproval({ ok: true, output: { stdout: { error: { code: "APPROVAL_PENDING", details: { approval: "A1" } } } } })).toBeUndefined();
    void (null as unknown as FinError);
  });
});
