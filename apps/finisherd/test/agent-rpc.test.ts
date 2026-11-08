// The agent RPCs over a real server (plan 7A Task 12): each RPC's happy path and its codes, the
// providers overview, the OpenRouter key and the local endpoint (human-only, no value in any
// answer), the closed-model-targets counsel gate, and crash recovery before the first request.
// Sessions run on the scripted `fake` harness, which exists only in a test-hooks build run with
// `FIN_TEST_HOOKS=1`.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Approval, CredentialFile, Node, Run, Session, WatchItem } from "@finisher/contracts";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { FakeScript } from "../src/agent/fake.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;
const hooks = process.env.FIN_TEST_HOOKS;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
const done = { type: "turnCompleted", stopReason: "endTurn" };
/** Turn 1 answers and ends; turn 2 waits for an answer that never comes (the run stays open). */
const SCRIPT: FakeScript = { name: "main", turns: [[{ emit: [msg("hello"), done] }], [{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-rpc-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData, { timers: false, fakeScripts: [SCRIPT] });
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

async function project(local: NdjsonClient, name = "P"): Promise<string> {
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, name), name, rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
  return projectId;
}

/** Waits until the session reaches `status` (the runtime applies provider events off the call). */
async function until(c: NdjsonClient, sessionId: string, status: Session["status"]): Promise<Session> {
  for (let i = 0; i < 200; i++) {
    const { session } = ok<{ session: Session }>(await c.call("agent.session.get", { sessionId }));
    if (session.status === status) return session;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error(`session ${sessionId} never became ${status}`);
}

/** Every item of a watch until it ends (or `n` items, then it is interrupted). */
async function watchAll(c: NdjsonClient, payload: Record<string, unknown>, n = Infinity): Promise<{ items: WatchItem[]; exit: Exit }> {
  const items: WatchItem[] = [];
  const s = c.stream("agent.watch", payload, (v) => {
    items.push(v as WatchItem);
    if (items.length >= n) c.interrupt(s.id);
  });
  return { items, exit: await s.done };
}

const start = (c: NdjsonClient, projectId: string, more: Record<string, unknown> = {}) =>
  c.call("agent.session.start", { projectId, harness: "fake", ...more });

describe("agent session RPCs", () => {
  test("start, prompt, steer, interrupt, resume, end, list, get, watch and delete", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const started = ok<{ session: Session; run?: Run; warnings?: { code: string }[] }>(await start(local, projectId, { prompt: "say hello", title: "first" }));
    expect(started.session).toMatchObject({ projectId, harness: "fake", role: "editor", mode: "draft", title: "first" });
    expect(started.session.rootBranch).toBeDefined();
    expect(started.run?.trigger).toBe("prompt");
    expect(started.warnings?.map((w) => w.code)).toEqual(["SANDBOX_DISABLED"]);
    const id = started.session.id;
    await until(local, id, "idle");
    // Turn 2 stays open: a prompt is refused while it runs, a steer is taken.
    const { run } = ok<{ run: Run }>(await local.call("agent.session.prompt", { sessionId: id, text: "keep going" }));
    expect(run.trigger).toBe("prompt");
    await until(local, id, "running");
    expect(code(await local.call("agent.session.prompt", { sessionId: id, text: "again" }))).toBe("SESSION_BUSY");
    expect(ok<{ queued: boolean }>(await local.call("agent.session.steer", { sessionId: id, text: "faster" }))).toEqual({ queued: false });
    expect(ok<{ session: Session }>(await local.call("agent.session.interrupt", { sessionId: id })).session.status).toBe("interrupted");
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: id })).session.status).toBe("idle");
    // Lists and reads.
    const listed = ok<{ sessions: Session[] }>(await local.call("agent.session.list", { projectId }));
    expect(listed.sessions.map((s) => s.id)).toEqual([id]);
    expect(ok<{ sessions: Session[] }>(await local.call("agent.session.list", { status: "ended" })).sessions).toEqual([]);
    const got = ok<{ session: Session; runs: Run[]; approvals: Approval[] }>(await local.call("agent.session.get", { sessionId: id }));
    expect(got.runs.map((r) => r.status)).toEqual(["completed", "interrupted"]);
    expect(got.approvals).toEqual([]);
    expect(ok<{ approvals: Approval[] }>(await local.call("agent.approval.list", { sessionId: id })).approvals).toEqual([]);
    // End: the branch stays; driving it again is SESSION_ENDED.
    const ended = ok<{ session: Session }>(await local.call("agent.session.end", { sessionId: id })).session;
    expect(ended).toMatchObject({ status: "ended", rootBranch: started.session.rootBranch });
    expect(code(await local.call("agent.session.prompt", { sessionId: id, text: "more" }))).toBe("SESSION_ENDED");
    // The watch of an ended session: its nodes, then the status, then the end of the stream.
    const all = await watchAll(local, { sessionId: id });
    expect(all.exit.ok).toBe(true);
    const nodes = all.items.flatMap((i) => (i.type === "nodeAdded" ? [i.node] : []));
    expect(nodes.map((n) => n.kind)).toContain("assistantMessage");
    expect(all.items.at(-1)).toMatchObject({ type: "statusChanged", status: "ended" });
    // `fromNode` is a node id: the watch starts after it.
    const after = await watchAll(local, { sessionId: id, fromNode: nodes[0]!.id });
    expect((after.items.find((i) => i.type === "nodeAdded") as { node: Node }).node.seq).toBe(nodes[0]!.seq + 1);
    expect(code((await watchAll(local, { sessionId: id, fromNode: "01zzzzzzzzzzzzzzzzzzzzzzzz" })).exit)).toBe("NOT_FOUND");
    // Delete is the owner's, with the challenge code: refused for `local`, done by the console.
    expect(code(await local.call("agent.session.delete", { sessionId: id }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    const owner = await server.console();
    expect(ok(await confirmed(owner, "agent.session.delete", { sessionId: id }))).toEqual({ deleted: true });
    expect(code(await local.call("agent.session.get", { sessionId: id }))).toBe("SESSION_NOT_FOUND");
  });

  test("the start codes: SESSION_NOT_FOUND, ROLE_UNKNOWN, MODE_INVALID, HARNESS_UNKNOWN, MODEL_UNKNOWN, PROVIDER_*", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    for (const tag of ["agent.session.get", "agent.session.interrupt", "agent.session.resume", "agent.session.end"]) {
      expect(code(await local.call(tag, { sessionId: "01zzzzzzzzzzzzzzzzzzzzzzzz" })), tag).toBe("SESSION_NOT_FOUND");
    }
    expect(code(await local.call("agent.session.prompt", { sessionId: "01zzzzzzzzzzzzzzzzzzzzzzzz", text: "x" }))).toBe("SESSION_NOT_FOUND");
    expect(code(await start(local, projectId, { role: "nobody" }))).toBe("ROLE_UNKNOWN");
    // The critic's ceiling is draft; the editor's default is draft.
    const tooHigh = await start(local, projectId, { role: "critic", mode: "auto" });
    expect(code(tooHigh)).toBe("MODE_INVALID");
    expect(tooHigh.ok ? undefined : tooHigh.error.details).toMatchObject({ mode: "auto", ceiling: "draft", role: "critic" });
    expect(code(await start(local, projectId, { harness: "codex" }))).toBe("HARNESS_UNKNOWN");
    // The built-in agent: an id that is not OpenRouter's; no key; no local endpoint.
    expect(code(await start(local, projectId, { harness: "builtin", model: "not a model" }))).toBe("MODEL_UNKNOWN");
    const noKey = await start(local, projectId, { harness: "builtin" });
    expect(code(noKey)).toBe("PROVIDER_AUTH");
    const noLocal = await start(local, projectId, { harness: "builtin", provider: "local", model: "llama3.2" });
    expect(code(noLocal)).toBe("PROVIDER_UNAVAILABLE");
    expect(noLocal.ok ? undefined : noLocal.error.fix).toContain("provider local set");
    // Nothing was made by the refusals.
    expect(ok<{ sessions: Session[] }>(await local.call("agent.session.list", {})).sessions).toEqual([]);
  });

  test("a session out of the caller's project reach is not found", async () => {
    await server.start();
    const local = await server.local();
    const [a, b] = [await project(local, "A"), await project(local, "B")];
    const theirs = ok<{ session: Session }>(await start(local, b)).session;
    const ours = ok<{ session: Session }>(await start(local, a)).session;
    // A credential the owner minted to read sessions of project A only.
    const owner = await server.console();
    const minted = ok<{ file: CredentialFile }>(await confirmed(owner, "auth.credential.mint", { kind: "agent", label: "reader", scopes: ["agent.read", "server.read"], projects: [a] }));
    const { client: reader, exit } = await server.proveAs(minted.file);
    ok(exit);
    expect(ok<{ session: Session }>(await reader.call("agent.session.get", { sessionId: ours.id })).session.id).toBe(ours.id);
    // Out of reach answers exactly what a missing session does.
    const hidden = await reader.call("agent.session.get", { sessionId: theirs.id });
    const missing = await reader.call("agent.session.get", { sessionId: "01zzzzzzzzzzzzzzzzzzzzzzzz" });
    expect(code(hidden)).toBe("SESSION_NOT_FOUND");
    expect(hidden.ok ? undefined : hidden.error.message).toBe(missing.ok ? undefined : missing.error.message);
    expect(ok<{ sessions: Session[] }>(await reader.call("agent.session.list", {})).sessions.map((s) => s.id)).toEqual([ours.id]);
    expect(code((await watchAll(reader, { sessionId: theirs.id })).exit)).toBe("SESSION_NOT_FOUND");
    expect(code(await reader.call("agent.approval.list", { sessionId: theirs.id }))).toBe("SESSION_NOT_FOUND");
    // A session's own credential never drives one (7A: sessions are driven by people).
    const mine = await open(server.core, local, a);
    const agent = await server.agent(mine.credentialPath);
    expect(code(await agent.call("agent.session.end", { sessionId: mine.session.id }))).toBe("SCOPE_DENIED");
  });

  test("start-up recovery runs before the first request is served", async () => {
    await server.start();
    let local = await server.local();
    const projectId = await project(local);
    const id = ok<{ session: Session }>(await start(local, projectId, { prompt: "one" })).session.id;
    await until(local, id, "idle");
    ok(await local.call("agent.session.prompt", { sessionId: id, text: "two" }));
    await until(local, id, "running");
    await server.stop();
    await server.start();
    local = await server.local();
    // The very first answer already sees the session interrupted by the restart.
    const { session } = ok<{ session: Session }>(await local.call("agent.session.get", { sessionId: id }));
    expect(session).toMatchObject({ status: "interrupted", statusReason: "serverRestart" });
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: id })).session.status).toBe("idle");
  });

  test("the fake harness exists only with FIN_TEST_HOOKS=1", async () => {
    process.env.FIN_TEST_HOOKS = "0";
    try {
      await server.start();
    } finally {
      process.env.FIN_TEST_HOOKS = "1";
    }
    const local = await server.local();
    const projectId = await project(local);
    const r = ok<{ providers: { harness: string }[] }>(await local.call("agent.providers", {}));
    expect(r.providers.map((p) => p.harness)).toEqual(["builtin"]);
    expect(code(await start(local, projectId))).toBe("HARNESS_UNKNOWN");
  });
});

/** OpenRouter's cached model list, fresh, so the planner does not refresh it. */
function seedListing(serverData: string, now: number): void {
  const price = { input: 1_000_000, output: 2_000_000, cacheRead: 0, cacheWrite: 0 };
  const model = (id: string, tools: boolean) => ({ id, window: 32_768, maxOutput: 4096, price, tools, thinking: false, images: false, openWeights: true });
  mkdirSync(join(serverData, "agents"), { recursive: true, mode: 0o700 });
  writeFileSync(join(serverData, "agents", "openrouter-models.json"), JSON.stringify({ v: 1, fetchedAt: now, models: [model("test/priced", true), model("test/no-tools", false)] }));
}

/** A loopback OpenAI-compatible model list (`GET /v1/models`). */
async function modelList(models: string[]): Promise<{ url: string; server: Server }> {
  const s = createServer((req, res) => {
    if (req.url === "/v1/models") res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data: models.map((id) => ({ id })) }));
    else res.writeHead(404).end();
  });
  await new Promise<void>((r) => s.listen(0, "127.0.0.1", r));
  return { url: `http://127.0.0.1:${(s.address() as AddressInfo).port}/v1`, server: s };
}

describe("providers, keys, the local endpoint and the counsel gate", () => {
  test("agent.providers: builtin with whether keys are set, fake in test builds, never a key or an address", async () => {
    await server.start();
    const local = await server.local();
    const owner = await server.console();
    const [a, b] = [await project(local, "A"), await project(local, "B")];
    type Providers = { providers: { harness: string; detected: boolean }[]; openrouter: { keySet: boolean; projects?: string[] }; local: { configured: boolean; reachable?: boolean; preset?: string } };
    const before = ok<Providers>(await local.call("agent.providers", {}));
    expect(before.providers.map((p) => p.harness).sort()).toEqual(["builtin", "fake"]);
    expect(before.openrouter).toEqual({ keySet: false, projects: [] });
    expect(before.local).toEqual({ configured: false });
    const KEY = "sk-or-v1-0123456789abcdef";
    const set = ok<{ key: { id: string; projectId?: string } }>(await confirmed(owner, "agent.key.set", { provider: "openrouter", key: KEY, projectId: a }));
    expect(set.key.projectId).toBe(a);
    const list = { url: "", server: undefined as Server | undefined };
    ({ url: list.url, server: list.server } = await modelList(["llama3.2"]));
    try {
      ok(await confirmed(owner, "agent.local.set", { endpoint: list.url }));
      const after = await local.call("agent.providers", {});
      const answer = ok<Providers>(after);
      expect(answer.openrouter).toEqual({ keySet: false, projects: [a] });
      expect(answer.local).toEqual({ configured: true, preset: "custom", reachable: true });
      const text = JSON.stringify(after);
      expect(text).not.toContain(KEY);
      expect(text).not.toContain(new URL(list.url).port);
      // A credential limited to project B does not learn A has a key of its own.
      const limited = await server.agent((await open(server.core, local, b)).credentialPath);
      expect(ok<Providers>(await limited.call("agent.providers", {})).openrouter).toEqual({ keySet: false, projects: [] });
      // `agent.key.list` names the key by id and label only.
      const keys = await confirmed(owner, "agent.key.list", {});
      expect(JSON.stringify(keys)).not.toContain(KEY);
      expect(ok<{ keys: { id: string }[] }>(keys).keys.map((k) => k.id)).toEqual([set.key.id]);
      // The local preset starts once its model is listed; no key is looked up.
      const s = ok<{ session: Session }>(await start(local, a, { harness: "builtin", provider: "local", model: "llama3.2" })).session;
      expect(s).toMatchObject({ provider: "local", model: "llama3.2" });
      expect(code(await start(local, a, { harness: "builtin", provider: "local", model: "unlisted-model" }))).toBe("MODEL_UNKNOWN");
    } finally {
      await new Promise((r) => list.server?.close(r));
    }
    // The server stopped answering: providers says so, `test` names no address.
    const gone = ok<Providers>(await local.call("agent.providers", {}));
    expect(gone.local).toEqual({ configured: true, preset: "custom", reachable: false });
    const t = await owner.call("agent.local.test", {});
    expect(ok<{ reachable: boolean }>(t).reachable).toBe(false);
    expect(JSON.stringify(t)).not.toContain(new URL(list.url).port);
    // `show` is the owner's own: the URL is there.
    expect(ok<{ baseUrl?: string }>(await owner.call("agent.local.show", {})).baseUrl).toBe(list.url);
    expect(ok(await confirmed(owner, "agent.local.remove", {}))).toEqual({ removed: true });
    expect(ok<Providers>(await local.call("agent.providers", {})).local).toEqual({ configured: false });
  });

  test("keys: only OpenRouter's, only by a person with the challenge code, never read back", async () => {
    seedListing(server.serverData, server.time.now());
    await server.start();
    const local = await server.local();
    const owner = await server.console();
    const projectId = await project(local);
    const anthropic = await confirmed(owner, "agent.key.set", { provider: "anthropic", key: "sk-ant-0123456789" });
    expect(code(anthropic)).toBe("PROVIDER_UNAVAILABLE");
    const forLocal = await confirmed(owner, "agent.key.set", { provider: "local", key: "anything-long" });
    expect(code(forLocal)).toBe("PROVIDER_UNAVAILABLE");
    expect(forLocal.ok ? undefined : forLocal.error.fix).toContain("provider local set");
    // A key the store refuses is never quoted back (the schema does not bound it: its refusal would).
    const huge = `sk-or-${"x".repeat(5000)}`;
    const tooLong = await confirmed(owner, "agent.key.set", { provider: "openrouter", key: huge });
    expect(code(tooLong)).toBe("SECRET_INVALID");
    expect(JSON.stringify(tooLong)).not.toContain("xxxxxxxx");
    // No challenge code: refused before anything is stored.
    expect(code(await owner.call("agent.key.set", { provider: "openrouter", key: "sk-or-v1-abcdefgh01" }))).toBe("CONFIRMATION_REQUIRED");
    // `local` (fin) and an agent session are never allowed, code or not.
    const agent = await server.agent((await open(server.core, local, projectId)).credentialPath);
    for (const c of [local, agent]) {
      for (const [tag, payload] of [
        ["agent.key.set", { provider: "openrouter", key: "sk-or-v1-abcdefgh01" }],
        ["agent.key.list", {}],
        ["agent.local.set", { endpoint: "ollama" }],
        ["agent.local.show", {}],
        ["training.closedTargets.set", { state: "cleared" }],
      ] as const) {
        expect(code(await c.call(tag, { ...payload, challenge: "ZZZZZZ" })), tag).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
      }
    }
    expect(existsSync(join(server.serverData, "secrets.json"))).toBe(false);
    const saved = ok<{ key: { id: string } }>(await confirmed(owner, "agent.key.set", { provider: "openrouter", key: "sk-or-v1-abcdefgh01", label: "main" }));
    // The key is found now. OpenRouter's listing (seeded before the start: tests never fetch it)
    // prices one model and lists another without tools; an unlisted model has no price.
    expect(code(await start(local, projectId, { harness: "builtin", model: "test/unlisted" }))).toBe("PRICING_UNKNOWN");
    const noTools = await start(local, projectId, { harness: "builtin", model: "test/no-tools" });
    expect(noTools.ok ? undefined : noTools.error).toMatchObject({ code: "MODEL_UNKNOWN", details: { reason: "noTools" } });
    const priced = ok<{ session: Session }>(await start(local, projectId, { harness: "builtin", model: "test/priced" })).session;
    expect(priced).toMatchObject({ harness: "builtin", provider: "openrouter", model: "test/priced", weights: "open" });
    expect(ok(await confirmed(owner, "agent.key.remove", { id: saved.key.id }))).toEqual({ removed: true });
    expect(ok(await confirmed(owner, "agent.key.remove", { id: saved.key.id }))).toEqual({ removed: false });
    const audit = ok<{ entries: { kind: string; details: Record<string, unknown> }[] }>(await owner.call("auth.audit.list", {}));
    const writes = audit.entries.filter((e) => e.kind === "settings.write").map((e) => e.details.rpc);
    expect(writes).toEqual(expect.arrayContaining(["agent.key.set", "agent.key.remove"]));
    expect(JSON.stringify(audit)).not.toContain("sk-or-v1-abcdefgh01");
  });

  test("a key is trimmed, then held to the console's rule: 8–1024 printable characters (L1)", async () => {
    await server.start();
    const owner = await server.console();
    for (const [what, key] of [
      ["spaces only", "          "],
      ["short once trimmed", "   short   "],
      ["inner space", "sk-or-v1 0123456789abcdef"],
      ["a newline inside", "sk-or-v1-0123\n456789abcdef"],
      ["a control character", "sk-or-v1-0123\u0007456789abcdef"],
      ["not ASCII", "sk-or-v1-0123456789abcdéf"],
      ["too long once trimmed", `  ${"k".repeat(1025)}  `],
    ] as const) {
      const refused = await confirmed(owner, "agent.key.set", { provider: "openrouter", key });
      expect(code(refused), what).toBe("SECRET_INVALID");
      expect(JSON.stringify(refused), what).not.toContain(key.trim().slice(0, 12) || "unreachable-needle");
    }
    expect(ok<{ keys: unknown[] }>(await owner.call("agent.key.list", {})).keys).toEqual([]);
    // The padding goes; what is left obeys the rule (8 and 1024 are both allowed).
    ok(await confirmed(owner, "agent.key.set", { provider: "openrouter", key: "  sk-or-v1-0123456789abcdef \n" }));
    ok(await confirmed(owner, "agent.key.set", { provider: "openrouter", key: "k".repeat(8), label: "low" }));
    ok(await confirmed(owner, "agent.key.set", { provider: "openrouter", key: `\t${"k".repeat(1024)}\t`, label: "high" }));
    expect(ok<{ keys: unknown[] }>(await owner.call("agent.key.list", {})).keys.length).toBeGreaterThanOrEqual(1);
  });

  test("a schema refusal of agent.key.set never quotes the key (an over-long label)", async () => {
    await server.start();
    const owner = await server.console();
    const SENTINEL = "sk-or-v1-SENTINELKEY0123456789";
    const refused = await confirmed(owner, "agent.key.set", { provider: "openrouter", key: SENTINEL, label: "L".repeat(5000) });
    expect(refused.ok).toBe(false);
    expect(JSON.stringify(refused)).not.toContain(SENTINEL);
    expect(JSON.stringify(refused)).not.toContain("SENTINELKEY");
    // Nothing was stored, and the key is in no later answer.
    expect(ok<{ keys: unknown[] }>(await owner.call("agent.key.list", {})).keys).toEqual([]);
    const audit = JSON.stringify(await owner.call("auth.audit.list", {}));
    expect(audit).not.toContain("SENTINELKEY");
    expect(existsSync(join(server.serverData, "secrets.json"))).toBe(false);
  });

  test("the local endpoint is validated and holds no secret", async () => {
    await server.start();
    const owner = await server.console();
    for (const endpoint of ["ftp://127.0.0.1/v1", "http://user:pw@127.0.0.1:11434/v1", "not a url", "anthropic"]) {
      expect(code(await confirmed(owner, "agent.local.set", { endpoint })), endpoint).toBe("PROVIDER_UNAVAILABLE");
    }
    expect(ok(await confirmed(owner, "agent.local.set", { endpoint: "ollama" }))).toEqual({ preset: "ollama" });
    const net = ok<{ preset: string; warnings?: { code: string }[] }>(await confirmed(owner, "agent.local.set", { endpoint: "http://192.0.2.10:8080/v1" }));
    expect(net.warnings?.map((w) => w.code)).toEqual(["LOCAL_ENDPOINT_UNENCRYPTED"]);
    const settings = JSON.parse(readFileSync(join(server.serverData, "settings.json"), "utf8")) as { agents: { local: unknown } };
    expect(settings.agents.local).toEqual({ preset: "custom", baseUrl: "http://192.0.2.10:8080/v1" });
    expect(existsSync(join(server.serverData, "secrets.json"))).toBe(false);
    // The audit says `local` and the preset, never the address.
    const audit = JSON.stringify(ok(await owner.call("auth.audit.list", {})));
    expect(audit).not.toContain("192.0.2.10");
    expect(audit).not.toContain("11434");
  });

  test("closed_model_targets_gate", async () => {
    await server.start();
    const local = await server.local();
    const owner = await server.console();
    expect(ok(await local.call("training.closedTargets.get", {}))).toEqual({ state: "pendingCounsel" });
    expect(code(await owner.call("training.closedTargets.set", { state: "cleared" }))).toBe("CONFIRMATION_REQUIRED");
    expect(code(await local.call("training.closedTargets.set", { state: "cleared" }))).toMatch(/^(SCOPE_DENIED|HUMAN_ONLY)$/);
    expect(ok(await local.call("training.closedTargets.get", {}))).toEqual({ state: "pendingCounsel" });
    expect(ok(await confirmed(owner, "training.closedTargets.set", { state: "cleared" }))).toEqual({ state: "cleared" });
    expect(ok(await local.call("training.closedTargets.get", {}))).toEqual({ state: "cleared" });
    const entries = ok<{ entries: { kind: string; details: Record<string, unknown> }[] }>(await owner.call("auth.audit.list", {})).entries;
    expect(entries.find((e) => e.kind === "settings.write" && e.details.rpc === "training.closedTargets.set")?.details.value).toBe("cleared");
    // Back to the default removes the key; a hand-broken value reads as the default.
    ok(await confirmed(owner, "training.closedTargets.set", { state: "pendingCounsel" }));
    const file = join(server.serverData, "settings.json");
    expect((JSON.parse(readFileSync(file, "utf8")) as { training?: unknown }).training).toBeUndefined();
    writeFileSync(file, JSON.stringify({ training: { closedModelTargets: "yes please" } }));
    expect(ok(await local.call("training.closedTargets.get", {}))).toEqual({ state: "pendingCounsel" });
  });
});
