// A remembered allow lifts a matching ask (sub-project 7B Decisions 4 and 12, owner decision
// 2026-11-08): an exact `allow` a person's `remember` wrote lifts an `ask` from the local,
// project, user and role layers for the call it covers; never a `deny`, never a managed `ask`.
// The lift needs the server's own record of the remember (`agent_remembered`): a rule that only
// claims `approval:<id>` (a file, a hand-added rule) lifts nothing.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ListedRule } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BUNDLED_ROLES } from "../src/agent/roles.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let dir: string;
let server: AgentServer;
let n = 0;
let started = false;
const start = async () => {
  await server.start();
  started = true;
};
const id = () => `01T${String(++n).padStart(23, "0")}`;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-lift-"));
  const serverData = join(dir, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
  started = false;
});

afterEach(async () => {
  await server.stop();
  rmSync(dir, { recursive: true, force: true });
});

const details = (e: Exit) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));
const checkpoint = (c: NdjsonClient, projectId: string, branch: string, name = "c1") => c.call("history.checkpoint", { projectId, branch, name, txId: id(), author: "x" });
const answer = (owner: NdjsonClient, approvalId: string, decision: "allow" | "deny", remember?: string) =>
  confirmed(owner, "agent.approval.answer", { id: approvalId, decision, ...(remember === undefined ? {} : { remember }) });
const settings = (file: string, rules: unknown[]) => writeFileSync(join(server.serverData, file), JSON.stringify({ agents: { rules } }));

/** A project and a running session; `ask` adds a local `ask checkpoint` for the session. */
async function setUp(o: { ask?: boolean; projectDir?: string } = {}) {
  if (!started) await start();
  const local = await server.local();
  const projectDir = o.projectDir ?? join(dir, `P${++n}`);
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const owner = await server.console();
  const s = await session(local, owner, projectId, o.ask ?? true);
  return { local, owner, projectId, projectDir, ...s };
}

async function session(local: NdjsonClient, owner: NdjsonClient, projectId: string, ask = true) {
  const opened = await open(server.core, local, projectId);
  if (ask) ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "checkpoint", sessionId: opened.session.id }));
  server.core.store.startRun(opened.session.id, { trigger: "prompt" });
  const a = await server.agent(opened.credentialPath);
  return { session: opened.session, a, root: opened.session.rootBranch! };
}

/** The layer and rule of the card an ask opened. */
const asked = (e: Exit) => {
  const subject = server.core.store.approval(String(details(e)?.approval))?.subject as { layer?: string; rule?: string; command?: string } | undefined;
  return { code: code(e), layer: subject?.layer, rule: subject?.rule, command: subject?.command };
};

/** A project folder holding `rules` as its `finisher.rules.json`. */
function projectFolder(rules: unknown[]): string {
  const d = join(dir, `F${++n}`);
  mkdirSync(d);
  writeFileSync(join(d, "finisher.rules.json"), JSON.stringify({ rules }));
  return d;
}

/** Trusts a project's rules file as it is now. */
async function trust(owner: NdjsonClient, projectId: string) {
  const { rulesHash } = ok<{ rulesHash: string }>(await owner.call("agent.rules.list", { projectId }));
  ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rulesHash }));
}

/** Asks, has a person allow with `remember`, and answers the approval id. */
async function rememberAllow(owner: NdjsonClient, a: NdjsonClient, projectId: string, root: string, scope: string) {
  const first = await checkpoint(a, projectId, root, "c1");
  expect(code(first)).toBe("APPROVAL_PENDING");
  const approvalId = String(details(first)!.approval);
  const r = ok<{ rule: ListedRule; notice?: string }>(await answer(owner, approvalId, "allow", scope));
  return { approvalId, rule: r.rule, notice: r.notice };
}

describe("a remembered allow lifts a matching ask", () => {
  test("session: the second identical call goes through without a card, and so do others of the command; no notice", async () => {
    const { owner, projectId, a, root } = await setUp();
    const { approvalId, notice } = await rememberAllow(owner, a, projectId, root, "session");
    expect(notice).toBeUndefined();
    // The identical request (a checkpoint name is used once: the rest are others of the command).
    for (const name of ["c1", "c2", "c3"]) expect(code(await checkpoint(a, projectId, root, name)), name).toBeUndefined();
    // The lift let them through: the answer's one-time grant was never needed.
    expect(server.core.store.approval(approvalId)!.consumedAt).toBeUndefined();
  });

  test("it lifts asks from the local, project (trusted), user and role layers together, for each scope", async () => {
    const editor = BUNDLED_ROLES.get("editor")!;
    const roleAsk = { id: "role.ask", effect: "ask" as const, command: "checkpoint", layer: "role" as const, source: "role:editor", createdAt: 0 };
    server.agentOptions = { ...server.agentOptions, roles: new Map([...BUNDLED_ROLES, ["editor", { ...editor, rules: [...editor.rules, roleAsk] }]]) };
    await start();
    settings("settings.json", [{ id: "u.ask", effect: "ask", command: "checkpoint" }]);
    for (const scope of ["session", "projectLocal", "project"]) {
      const { owner, projectId, a, root } = await setUp({ projectDir: projectFolder([{ id: "p.ask", effect: "ask", command: "checkpoint" }]) });
      await trust(owner, projectId);
      // Each layer asks on its own (the session's local ask is the first named).
      const { rule } = await rememberAllow(owner, a, projectId, root, scope);
      expect(rule.applies, scope).toBe(true);
      expect(code(await checkpoint(a, projectId, root, "c1")), scope).toBeUndefined();
      expect(code(await checkpoint(a, projectId, root, "again")), scope).toBeUndefined();
    }
  });

  test("each layer's ask alone is lifted (project, user, role)", async () => {
    const editor = BUNDLED_ROLES.get("editor")!;
    let roleRules = editor.rules;
    server.agentOptions = { ...server.agentOptions, roles: new Map([...BUNDLED_ROLES, ["editor", { ...editor, get rules() { return roleRules; } }]]) };
    await start();
    // Project only.
    const p = await setUp({ ask: false, projectDir: projectFolder([{ id: "p.ask", effect: "ask", command: "checkpoint" }]) });
    await trust(p.owner, p.projectId);
    await rememberAllow(p.owner, p.a, p.projectId, p.root, "session");
    expect(code(await checkpoint(p.a, p.projectId, p.root, "x"))).toBeUndefined();
    // User only.
    settings("settings.json", [{ id: "u.ask", effect: "ask", command: "checkpoint" }]);
    const u = await setUp({ ask: false });
    await rememberAllow(u.owner, u.a, u.projectId, u.root, "session");
    expect(code(await checkpoint(u.a, u.projectId, u.root, "x"))).toBeUndefined();
    writeFileSync(join(server.serverData, "settings.json"), "{}");
    // Role only.
    roleRules = [...editor.rules, { id: "role.ask", effect: "ask", command: "checkpoint", layer: "role", source: "role:editor", createdAt: 0 }];
    const r = await setUp({ ask: false });
    expect(asked(await checkpoint(r.a, r.projectId, r.root, "y"))).toMatchObject({ code: "APPROVAL_PENDING", layer: "role" });
    await rememberAllow(r.owner, r.a, r.projectId, r.root, "session");
    expect(code(await checkpoint(r.a, r.projectId, r.root, "x"))).toBeUndefined();
  });

  test("a managed ask stands; a deny is never lifted", async () => {
    const { owner, projectId, a, root, local } = await setUp();
    await rememberAllow(owner, a, projectId, root, "session");
    settings("managed-settings.json", [{ id: "m.ask", effect: "ask", command: "checkpoint" }]);
    expect(asked(await checkpoint(a, projectId, root, "m"))).toMatchObject({ code: "APPROVAL_PENDING", layer: "managed", rule: "m.ask" });
    writeFileSync(join(server.serverData, "managed-settings.json"), "{}");
    expect(code(await checkpoint(a, projectId, root, "free"))).toBeUndefined();
    // A deny from any layer refuses it.
    settings("settings.json", [{ id: "u.deny", effect: "deny", command: "checkpoint" }]);
    expect(details(await checkpoint(a, projectId, root, "u"))).toMatchObject({ rule: "u.deny", layer: "user" });
    writeFileSync(join(server.serverData, "settings.json"), "{}");
    const b = await session(local, owner, projectId);
    await rememberAllow(owner, b.a, projectId, b.root, "projectLocal");
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "checkpoint", sessionId: b.session.id }));
    expect(code(await checkpoint(b.a, projectId, b.root, "d"))).toBe("RULE_DENIED");
  });

  test("a hand-added allow lifts nothing; a remembered one covers only its command, session and project", async () => {
    const { owner, projectId, a, root, local, session: s } = await setUp();
    // A person's own allows (`agent.rules.add`) for the session and the project: the ask stands.
    ok(await confirmed(owner, "agent.rules.add", { effect: "allow", command: "checkpoint", sessionId: s.id }));
    ok(await confirmed(owner, "agent.rules.add", { effect: "allow", command: "checkpoint", projectId }));
    expect(code(await checkpoint(a, projectId, root, "h"))).toBe("APPROVAL_PENDING");
    // Another name of the same call (`rpc:history.checkpoint`) keeps its own ask.
    ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "rpc:history.checkpoint", sessionId: s.id }));
    await rememberAllow(owner, a, projectId, root, "session");
    expect(asked(await checkpoint(a, projectId, root, "rpc"))).toMatchObject({ code: "APPROVAL_PENDING", command: "rpc:history.checkpoint" });
    // Another session of the project: remembered for session A only.
    const other = await session(local, owner, projectId);
    expect(code(await checkpoint(other.a, projectId, other.root, "o"))).toBe("APPROVAL_PENDING");
    // projectLocal covers the project's sessions, and no other project's.
    await rememberAllow(owner, other.a, projectId, other.root, "projectLocal");
    expect(code(await checkpoint(other.a, projectId, other.root, "o2"))).toBeUndefined();
    const third = await session(local, owner, projectId);
    expect(code(await checkpoint(third.a, projectId, third.root, "t"))).toBeUndefined();
    const elsewhere = await setUp();
    expect(code(await checkpoint(elsewhere.a, elsewhere.projectId, elsewhere.root, "e"))).toBe("APPROVAL_PENDING");
  });

  test("a forged approval source lifts nothing: a settings or project rule claiming one, or reusing a remembered rule's id", async () => {
    const { owner, projectId, projectDir, a, root, local } = await setUp();
    const { approvalId, rule } = await rememberAllow(owner, a, projectId, root, "session");
    // Session B of the same project, with its own ask.
    const b = await session(local, owner, projectId);
    const forged = [
      { id: rule.id, effect: "allow", command: "checkpoint", source: `approval:${approvalId}` },
      { id: `approval:${approvalId}`, effect: "allow", command: "checkpoint" },
    ];
    settings("settings.json", forged);
    const file = join(projectDir, "finisher.rules.json");
    writeFileSync(file, JSON.stringify({ rules: forged }));
    await trust(owner, projectId);
    // The files' rules are read with their own source, whatever they claim.
    const now = ok<{ rules: ListedRule[] }>(await owner.call("agent.rules.list", { sessionId: b.session.id })).rules;
    expect(now.filter((r) => (r.layer === "user" || r.layer === "project") && r.effect === "allow").map((r) => r.source).sort()).toEqual(["finisher.rules.json", "finisher.rules.json", "settings.json", "settings.json"]);
    expect(code(await checkpoint(b.a, projectId, b.root, "f"))).toBe("APPROVAL_PENDING");
    // Nor does a local row that only claims the source (written by anything but `remember`).
    const claimed = server.agents.ruleStore.add({ id: "01FORGED000000000000000000", effect: "allow", command: "checkpoint", projectId, sessionId: b.session.id, source: `approval:${approvalId}`, createdAt: 1 });
    expect(claimed.source).toBe(`approval:${approvalId}`);
    expect(code(await checkpoint(b.a, projectId, b.root, "g"))).toBe("APPROVAL_PENDING");
    expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({ rules: forged });
  });

  test("a remembered project allow lifts while it is in the trusted file, and stops once a person takes it out", async () => {
    const { owner, projectId, projectDir, a, root } = await setUp({ projectDir: projectFolder([]) });
    await trust(owner, projectId);
    const { rule } = await rememberAllow(owner, a, projectId, root, "project");
    expect(rule).toMatchObject({ layer: "project", applies: true });
    expect(code(await checkpoint(a, projectId, root, "p"))).toBeUndefined();
    // The same rule copied into another project's (trusted) file proves nothing there.
    const q = await setUp({ projectDir: projectFolder([{ id: rule.id, effect: "allow", command: "checkpoint" }]) });
    await trust(q.owner, q.projectId);
    expect(code(await checkpoint(q.a, q.projectId, q.root, "q"))).toBe("APPROVAL_PENDING");
    writeFileSync(join(projectDir, "finisher.rules.json"), JSON.stringify({ rules: [] }));
    await trust(owner, projectId);
    expect(code(await checkpoint(a, projectId, root, "p2"))).toBe("APPROVAL_PENDING");
  });
});
