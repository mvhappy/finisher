// The agent RPCs' schemas and scopes (plan 7A Task 2; spec §3.5).
import { Effect, Schema } from "effect";
import { RpcSchema } from "effect/rpc";
import { describe, expect, it } from "vitest";
import {
  Approval,
  AgentSessionStartPayload,
  AgentSelfTryPayload,
  AgentWatchPayload,
  Node,
  Run,
  Session,
  WATCH_ITEM_TYPES,
  WatchItem,
} from "./agent.ts";
import { ProviderKey } from "./agent-admin.ts";
import { defaultScopes } from "./auth.ts";
import { MINT_DEFAULT_SCOPES } from "./credentials.ts";
import { STREAM_RPCS } from "./diagnostics.ts";
import { FinRpcs } from "./rpc.ts";
import { HUMAN_ONLY, needsConfirmation, requiredScopes, ruleOf, SCOPES, WRITE_RPCS } from "./scopes.ts";

const usage = { input: 10, output: 2, cacheRead: 100, cacheWrite: 0, reasoning: 1, moneyMicros: 42, costBasis: "reported", estimated: false } as const;
const session = {
  id: "s1",
  projectId: "p1",
  harness: "builtin",
  provider: "openrouter",
  model: "openai/gpt-oss-120b",
  weights: "open",
  role: "editor",
  mode: "draft",
  rootBranch: "agent/editor-1",
  base: { branch: "main", seq: 12 },
  status: "idle",
  credentialId: "s1",
  budget: { tokens: 2_000_000, moneyMicros: 10_000_000 },
  usage,
  createdBy: { principal: { kind: "console", id: "console" }, actor: { kind: "person", id: "u1" } },
  createdAt: 1_700_000_000_000,
  title: "Trim the intro",
} as const;
const run = { id: "r1", sessionId: "s1", seq: 1, trigger: "prompt", status: "completed", startedAt: 1, endedAt: 2, headAtStart: 12, stopReason: "endTurn", usage } as const;
const node = { id: "n1", runId: "r1", sessionId: "s1", seq: 3, kind: "toolCall", providerId: "call_1", callId: "c1", payload: { argv: ["view"] }, createdAt: 3 } as const;
const approval = {
  id: "a1",
  sessionId: "s1",
  kind: "rule",
  status: "answered",
  subject: { command: "render", args: { preset: "web" } },
  argsHash: "00ff",
  requestedAt: 1,
  expiresAt: 1_800_001,
  decision: "allow",
  by: "console:console",
  answeredAt: 5,
} as const;
const budget = { tokens: { limit: 100, used: 10 }, moneyMicros: { limit: 1000, used: 10 }, wallMs: { limit: 60_000, used: 5 } } as const;

const watchSamples: WatchItem[] = [
  { type: "nodeAdded", node },
  { type: "nodeAdded", node: { id: "n2", runId: "r1", sessionId: "s1", seq: 4, kind: "toolResult", blobRef: "ab".repeat(32), createdAt: 4 } },
  { type: "delta", runId: "r1", kind: "message", text: "Trim" },
  { type: "delta", runId: "r1", kind: "toolOutput", text: "{", callId: "c1" },
  { type: "statusChanged", status: "interrupted", reason: "BUDGET_EXCEEDED", runId: "r1" },
  { type: "statusChanged", status: "idle" },
  { type: "usageUpdated", runId: "r1", usage, session: usage, budget },
  { type: "approvalOpened", approval: { ...approval, status: "open" } },
];

const roundTrip = <S extends Schema.Codec<unknown, unknown>>(schema: S, value: S["Type"]) => {
  const wire = JSON.parse(JSON.stringify(Schema.encodeSync(schema as Schema.Codec<unknown, unknown, never, never>)(value)));
  expect(Schema.decodeUnknownSync(schema as Schema.Codec<unknown, unknown, never, never>)(wire)).toEqual(value);
};

describe("schemas", () => {
  it("round-trip every WatchItem variant", () => {
    for (const w of watchSamples) roundTrip(WatchItem, w);
    expect([...new Set(watchSamples.map((w) => w.type))].sort()).toEqual([...WATCH_ITEM_TYPES].sort());
    expect(WatchItem.members.length).toBe(WATCH_ITEM_TYPES.length);
  });

  it("round-trip Session, Run, Node and Approval", () => {
    roundTrip(Session, session);
    roundTrip(Session, { ...session, harness: "claudeCode", provider: "anthropic", weights: "closed", status: "ended", statusReason: "ended", endedAt: 9, parent: "s0" });
    // `plan` mode creates no branch.
    const { rootBranch: _root, ...plan } = session;
    roundTrip(Session, { ...plan, mode: "plan" });
    roundTrip(Run, run);
    roundTrip(Run, { ...run, status: "running" });
    roundTrip(Node, node);
    roundTrip(Approval, approval);
    const { decision: _d, by: _b, answeredAt: _a, ...open } = approval;
    roundTrip(Approval, { ...open, status: "open" });
  });

  it("refuse what the contract forbids", () => {
    const decode = (schema: Schema.Codec<unknown, unknown, never, never>, v: unknown) => () => Schema.decodeUnknownSync(schema)(v);
    expect(decode(WatchItem as never, { type: "fork" })).toThrow();
    expect(decode(WatchItem as never, { type: "delta", runId: "r", kind: "plan", text: "x" })).toThrow();
    expect(decode(Session as never, { ...session, weights: "secret" })).toThrow();
    expect(decode(Session as never, { ...session, createdAt: -1 })).toThrow();
    expect(decode(Run as never, { ...run, status: "paused" })).toThrow();
    expect(decode(Approval as never, { ...approval, decision: "maybe" })).toThrow();
  });

  it("bound what a caller sends", () => {
    const start = (v: unknown) => () => Schema.decodeUnknownSync(AgentSessionStartPayload)(v);
    expect(start({ projectId: "p" })).not.toThrow();
    expect(start({ projectId: "p", harness: "builtin", provider: "local", model: "qwen3", mode: "plan", base: "main", budget: { tokens: 5 } })).not.toThrow();
    expect(start({ projectId: "p", provider: "anthropic" })).toThrow();
    expect(start({ projectId: "p", prompt: "" })).toThrow();
    expect(start({ projectId: "p", title: "x".repeat(201) })).toThrow();
    expect(start({ projectId: "p", attachments: Array.from({ length: 9 }, () => ({ type: "text", text: "x" })) })).toThrow();
    expect(start({})).toThrow();
    expect(Schema.decodeUnknownSync(AgentWatchPayload)({ sessionId: "s", fromNode: "n1" })).toEqual({ sessionId: "s", fromNode: "n1" });
    const tried = (v: unknown) => () => Schema.decodeUnknownSync(AgentSelfTryPayload)(v);
    for (const action of ["record", "setDefault", "clear"]) expect(tried({ action, branch: "try-1" })).not.toThrow();
    expect(tried({ action: "delete", branch: "try-1" })).toThrow();
    expect(tried({ action: "record" })).toThrow();
  });
});

const AGENT_RPCS = [
  "agent.session.start",
  "agent.session.prompt",
  "agent.session.steer",
  "agent.session.interrupt",
  "agent.session.resume",
  "agent.session.end",
  "agent.session.list",
  "agent.session.get",
  "agent.session.delete",
  "agent.watch",
  "agent.approval.list",
  "agent.approval.answer",
  "agent.providers",
  "agent.self",
  "agent.self.try",
] as const;
type AgentTag = (typeof AGENT_RPCS)[number];

/** The owner's agent settings (Task 12, `agent-admin.ts`): rule and whether a change takes the typed code. */
const ADMIN_RPCS = {
  "agent.key.set": { scopes: ["agent.admin"], confirmed: true },
  "agent.key.list": { scopes: ["agent.admin"], confirmed: false },
  "agent.key.remove": { scopes: ["agent.admin"], confirmed: true },
  "agent.local.set": { scopes: ["agent.admin"], confirmed: true },
  "agent.local.show": { scopes: ["agent.admin"], confirmed: false },
  "agent.local.test": { scopes: ["agent.admin"], confirmed: false },
  "agent.local.remove": { scopes: ["agent.admin"], confirmed: true },
  "training.closedTargets.get": { scopes: ["server.read"], confirmed: false },
  "training.closedTargets.set": { scopes: ["settings.admin"], confirmed: true },
} as const;

describe("the group", () => {
  it("holds exactly the agent RPCs of spec §3.5", () => {
    const admin = new Set<string>(Object.keys(ADMIN_RPCS));
    expect([...FinRpcs.requests.keys()].filter((t) => t.startsWith("agent.") && !admin.has(t)).sort()).toEqual([...AGENT_RPCS].sort());
  });

  it("the owner's agent settings are human-only, each change with the typed code, no key in an answer", () => {
    for (const [tag, want] of Object.entries(ADMIN_RPCS) as [keyof typeof ADMIN_RPCS, (typeof ADMIN_RPCS)[keyof typeof ADMIN_RPCS]][]) {
      expect(ruleOf(tag).scopes, tag).toEqual(want.scopes);
      expect(needsConfirmation(tag, {}), tag).toBe(want.confirmed);
      expect(WRITE_RPCS.has(tag), tag).toBe(false);
    }
    // A stored key is named by its metadata only: no field can carry the key.
    expect(Object.keys(ProviderKey.fields).sort()).toEqual(["createdAt", "id", "label", "projectId", "provider"]);
    for (const s of ["agent.admin", "settings.admin"] as const) expect(HUMAN_ONLY.has(s)).toBe(true);
  });

  it("rpc_stats_excludes_streams: agent.watch is the one new stream", () => {
    expect(STREAM_RPCS).toContain("agent.watch");
    const streams = [...FinRpcs.requests].filter(([, rpc]) => RpcSchema.isStreamSchema(rpc.successSchema)).map(([tag]) => tag);
    expect(streams.filter((t) => t.startsWith("agent."))).toEqual(["agent.watch"]);
  });

  it("encodes a watch item through the stream's success schema", async () => {
    const success = FinRpcs.requests.get("agent.watch")!.successSchema;
    const item = RpcSchema.isStreamSchema(success) ? success.success : success;
    const encode = (v: unknown) => Effect.runPromise(Schema.encodeUnknownEffect(item as Schema.Codec<unknown, unknown, never, never>)(v));
    for (const w of watchSamples) expect(await encode(w)).toEqual(JSON.parse(JSON.stringify(w)));
  });
});

/** Every agent RPC: its own scopes and who may call it (spec §3.5; plan 7A Task 2). */
const TABLE: Record<AgentTag, { scopes: string[]; kinds?: string[]; confirmed?: boolean }> = {
  "agent.session.start": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.prompt": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.steer": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.interrupt": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.resume": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.end": { scopes: ["agent.run"], kinds: ["local", "console", "device"] },
  "agent.session.list": { scopes: ["agent.read"] },
  "agent.session.get": { scopes: ["agent.read"] },
  "agent.session.delete": { scopes: ["agent.admin"], confirmed: true },
  "agent.watch": { scopes: ["agent.read"] },
  "agent.approval.list": { scopes: ["agent.read"] },
  "agent.approval.answer": { scopes: ["agent.approve"], confirmed: true },
  "agent.providers": { scopes: ["server.read"] },
  "agent.self": { scopes: ["project.read"], kinds: ["agent"] },
  "agent.self.try": { scopes: ["project.write"], kinds: ["agent"] },
};

describe("scopes", () => {
  it("every agent RPC has exactly the rule of the table", () => {
    for (const tag of AGENT_RPCS) {
      const rule = ruleOf(tag);
      const want = TABLE[tag];
      expect(rule.scopes, tag).toEqual(want.scopes);
      expect(rule.kinds, tag).toEqual(want.kinds);
      // None writes through the store's guard: a session's branch is made by `branch.create`.
      expect(WRITE_RPCS.has(tag), tag).toBe(false);
      expect(rule.targets, tag).toBeUndefined();
      expect(rule.unconfirmed, tag).toBeUndefined();
    }
  });

  it("adds the four agent scopes, two of them human-only", () => {
    for (const s of ["agent.read", "agent.run", "agent.approve", "agent.admin"] as const) expect(SCOPES).toContain(s);
    expect([...HUMAN_ONLY].filter((s) => s.startsWith("agent."))).toEqual(["agent.approve", "agent.admin"]);
    // `fin` (local) drives and reads sessions; it never answers an approval or deletes one.
    const local = defaultScopes("local");
    expect(local).toContain("agent.run");
    expect(local).toContain("agent.read");
    expect(local).not.toContain("agent.approve");
    expect(local).not.toContain("agent.admin");
    expect(defaultScopes("console")).toEqual(SCOPES);
    // No credential is minted with an agent scope unless asked (Decision 15.3: no mode grants them in 7A).
    expect(MINT_DEFAULT_SCOPES.filter((s) => s.startsWith("agent."))).toEqual([]);
    expect(defaultScopes("agent")).toEqual([]);
  });

  it("answering an approval and deleting a session are human-only and confirmed", () => {
    for (const tag of AGENT_RPCS.filter((t) => TABLE[t].confirmed === true)) {
      expect(requiredScopes(tag, {}).some((s) => HUMAN_ONLY.has(s)), tag).toBe(true);
      expect(needsConfirmation(tag, {}), tag).toBe(true);
      const rpc = FinRpcs.requests.get(tag) as unknown as { payloadSchema: { fields?: Record<string, unknown> } };
      expect(Object.keys(rpc.payloadSchema.fields ?? {}), tag).toContain("challenge");
    }
    // Nothing else of the group needs a person.
    for (const tag of AGENT_RPCS.filter((t) => TABLE[t].confirmed !== true)) {
      expect(needsConfirmation(tag, { mode: "plan" }), tag).toBe(false);
    }
  });

  it("a start needs project.write unless the session is plan", () => {
    expect(requiredScopes("agent.session.start", { projectId: "p", mode: "plan" })).toEqual(["agent.run", "project.read"]);
    for (const payload of [{ projectId: "p" }, { projectId: "p", mode: "draft" }, { projectId: "p", mode: "auto" }]) {
      expect(requiredScopes("agent.session.start", payload), JSON.stringify(payload)).toEqual(["agent.run", "project.read", "project.write"]);
    }
  });
});
