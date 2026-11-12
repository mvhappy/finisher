// Project trust for command rules (sub-project 7B Task 2, Decision 6; spec §9.4): a project's
// `finisher.rules.json` applies only once a person trusts it, and stops again when the file
// changes; local rules apply with no trust; trusting is human-only with the typed code. Also
// the console's `agent rules` and `project trust`, and agent migration 0004.
import { mkdirSync, mkdtempSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Rule } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { agent0001 } from "../src/control/migrations/agent/0001.ts";
import { agent0002 } from "../src/control/migrations/agent/0002.ts";
import { agent0003 } from "../src/control/migrations/agent/0003.ts";
import { agent0004 } from "../src/control/migrations/agent/0004.ts";
import "../src/agent/store.ts";
import { RULES_FILE } from "../src/agent/trust.ts";
import { CONTROL_CHAINS } from "../src/control/migrations.ts";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { person } from "./support/person.ts";
import { addAsset, commit, confirmed, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-trust-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

async function setUp() {
  await server.start();
  const local = await server.local();
  const dir = join(tmp, "P");
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const opened = await open(server.core, local, projectId);
  const a = await server.agent(opened.credentialPath);
  const owner = await server.console();
  return { local, owner, projectId, dir, session: opened.session, a };
}

const checkpoint = (c: NdjsonClient, projectId: string, branch: string) => c.call("history.checkpoint", { projectId, branch, name: `c${++n}`, txId: id(), author: "x" });
const writeRules = (dir: string, rules: unknown[]) => writeFileSync(join(dir, RULES_FILE), JSON.stringify({ rules }));
/** The file's hash as `agent.rules.list` shows it, then a trust bound to it. */
async function trust(owner: NdjsonClient, projectId: string) {
  const { rulesHash } = ok<{ rulesHash?: string }>(await owner.call("agent.rules.list", { projectId }));
  return confirmed(owner, "agent.project.trust", { projectId, expectedHash: rulesHash });
}
const projectRulesOf = async (c: NdjsonClient, sessionId: string) => ok<{ projectRules?: string }>(await c.call("agent.session.get", { sessionId })).projectRules;

describe("project trust", () => {
  test("project_rules_need_trust: no effect until trusted, again after a change; local rules need none", async () => {
    const { local, owner, projectId, dir, session, a } = await setUp();
    const root = session.rootBranch!;
    expect(await projectRulesOf(local, session.id)).toBe("none");
    writeRules(dir, [{ id: "p1", effect: "deny", command: "checkpoint" }]);
    // Untrusted: the deny has no effect, and the session says its project's rules are ignored.
    expect(code(await checkpoint(a, projectId, root))).toBeUndefined();
    expect(await projectRulesOf(local, session.id)).toBe("untrusted");
    const listed = ok<{ rules: Array<Rule & { applies: boolean }>; projectRules: string; rulesHash: string }>(await local.call("agent.rules.list", { projectId }));
    expect(listed.projectRules).toBe("untrusted");
    expect(listed.rules.find((r) => r.id === "p1")).toMatchObject({ layer: "project", applies: false });
    // Trusted by a person with the typed code: the deny applies.
    expect(code(await owner.call("agent.project.trust", { projectId, expectedHash: listed.rulesHash }))).toBe("CONFIRMATION_REQUIRED");
    const trusted = ok<{ rulesHash: string; rules: number }>(await trust(owner, projectId));
    expect(trusted).toMatchObject({ rules: 1, rulesHash: expect.stringMatching(/^sha256:[0-9a-f]{64}$/) });
    expect(await projectRulesOf(local, session.id)).toBe("trusted");
    const e = await checkpoint(a, projectId, root);
    expect(e.ok ? undefined : [e.error.code, (e.error.details as { rule: string; layer: string }).rule, (e.error.details as { layer: string }).layer]).toEqual(["RULE_DENIED", "p1", "project"]);
    // The file changes (an agent could edit it): ignored again until trusted again.
    writeRules(dir, [{ id: "p1", effect: "deny", command: "checkpoint" }, { id: "p2", effect: "allow", command: "**" }]);
    expect(await projectRulesOf(local, session.id)).toBe("changed");
    expect(code(await checkpoint(a, projectId, root))).toBeUndefined();
    ok(await trust(owner, projectId));
    // Trusted again: its own allow lifts nothing (deny wins).
    expect(code(await checkpoint(a, projectId, root))).toBe("RULE_DENIED");
    // A local rule applies with no trust at all.
    writeRules(dir, []);
    expect(await projectRulesOf(local, session.id)).toBe("changed");
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "checkpoint", projectId }));
    expect(code(await checkpoint(a, projectId, root))).toBe("RULE_DENIED");
    // A file that does not read as rules cannot be trusted.
    writeFileSync(join(dir, RULES_FILE), "{ nope");
    expect(code(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none" }))).toBe("INVALID_REQUEST");
  });

  test("trust_binds_what_the_person_saw: a file changed after it was listed is not trusted", async () => {
    const { local, owner, projectId, dir, session } = await setUp();
    writeRules(dir, [{ id: "p1", effect: "deny", command: "checkpoint" }]);
    const { rulesHash: seen } = ok<{ rulesHash: string }>(await owner.call("agent.rules.list", { projectId }));
    expect(seen).toMatch(/^sha256:[0-9a-f]{64}$/);
    // The file changes between the listing and the typed code.
    writeRules(dir, [{ id: "p1", effect: "allow", command: "checkpoint" }]);
    const e = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: seen });
    expect(e.ok ? undefined : [e.error.code, e.error.details]).toEqual(["INVALID_REQUEST", expect.objectContaining({ field: "expectedHash", reason: "changed" })]);
    expect(await projectRulesOf(local, session.id)).toBe("untrusted");
    // No file: `none`, and a file appearing since is a change too.
    unlinkSync(join(dir, RULES_FILE));
    expect(ok<{ rulesHash: string }>(await owner.call("agent.rules.list", { projectId })).rulesHash).toBe("none");
    writeRules(dir, []);
    expect(code(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none" }))).toBe("INVALID_REQUEST");
    ok(await trust(owner, projectId));
    expect(await projectRulesOf(local, session.id)).toBe("trusted");
  });

  test("a trusted file deleted or unreadable is changed, with the reason", async () => {
    const { local, owner, projectId, dir, session } = await setUp();
    writeRules(dir, [{ id: "p1", effect: "deny", command: "checkpoint" }]);
    ok(await trust(owner, projectId));
    unlinkSync(join(dir, RULES_FILE));
    expect(await projectRulesOf(local, session.id)).toBe("changed");
    expect(ok<{ projectRules: string; projectRulesReason?: string }>(await local.call("agent.rules.list", { projectId }))).toMatchObject({ projectRules: "changed", projectRulesReason: "deleted" });
    // Not a file it can read (a folder in its place).
    mkdirSync(join(dir, RULES_FILE));
    expect(ok<{ projectRules: string; projectRulesReason?: string }>(await local.call("agent.rules.list", { projectId }))).toMatchObject({ projectRules: "changed", projectRulesReason: "unreadable" });
  });

  test("a rules file with a pattern that matches nothing is refused, naming the rule", async () => {
    const { owner, projectId, dir } = await setUp();
    writeRules(dir, [{ id: "ok", effect: "deny", command: "render" }, { id: "shouty", effect: "deny", command: "Render" }]);
    const e = await trust(owner, projectId);
    expect(e.ok ? undefined : e.error.code).toBe("INVALID_REQUEST");
    expect(e.ok ? "" : e.error.message).toContain("shouty");
    expect(code(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "Render" }))).toBe("INVALID_REQUEST");
  });

  test("rule and trust changes are audited agent.rule.add|remove and agent.project.trust", async () => {
    const { owner, projectId, dir, session } = await setUp();
    const { rule } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render", sessionId: session.id }));
    ok(await confirmed(owner, "agent.rules.remove", { id: rule.id }));
    writeRules(dir, []);
    const { rulesHash } = ok<{ rulesHash: string }>(await trust(owner, projectId));
    const { entries } = ok<{ entries: Array<{ kind: string; details: Record<string, unknown> }> }>(await owner.call("auth.audit.list", {}));
    const of = (kind: string) => entries.find((x) => x.kind === kind)?.details;
    expect(of("agent.rule.add")).toEqual({ rpc: "agent.rules.add", ruleId: rule.id, layer: "local", value: "deny", projectId, sessionId: session.id });
    expect(of("agent.rule.remove")).toEqual({ rpc: "agent.rules.remove", ruleId: rule.id, layer: "local", value: "deny", projectId, sessionId: session.id });
    expect(of("agent.project.trust")).toEqual({ rpc: "agent.project.trust", projectId, rulesHash });
  });

  test("a session's own local rules go with the session", async () => {
    const { owner, session } = await setUp();
    const { rule } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render", sessionId: session.id }));
    const { rule: kept } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render", projectId: session.projectId }));
    ok(await confirmed(owner, "agent.session.delete", { sessionId: session.id }));
    const ids = ok<{ rules: Rule[] }>(await owner.call("agent.rules.list", {})).rules.map((r) => r.id);
    expect(ids).not.toContain(rule.id);
    expect(ids).toContain(kept.id);
  });

  test("untrusted_project_rules_do_not_apply: its deny has no effect; none of its rules is listed as applying", async () => {
    // Only the deny half can be observed: under deny-wins an `allow` never changes an outcome
    // (no match is already `allow`, and an allow lifts no other layer's ask or deny), so an
    // ignored allow and an applied one decide alike. The listing still says neither applies.
    const { local, projectId, dir, session, a } = await setUp();
    writeRules(dir, [
      { id: "p-deny", effect: "deny", command: "checkpoint" },
      { id: "p-allow", effect: "allow", command: "asset.add" },
    ]);
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBeUndefined();
    const { rules } = ok<{ rules: Array<Rule & { applies: boolean }> }>(await local.call("agent.rules.list", { sessionId: session.id }));
    expect(rules.filter((r) => r.layer === "project").map((r) => [r.id, r.applies])).toEqual([["p-deny", false], ["p-allow", false]]);
  });

  test("trusting is refused for agent and local principals", async () => {
    const { local, projectId, a } = await setUp();
    for (const c of [a, local]) {
      const e = await c.call("agent.project.trust", { projectId, expectedHash: "none" });
      expect(e.ok ? undefined : [e.error.code, (e.error.details as { reason?: string }).reason]).toEqual(["SCOPE_DENIED", "principalKind"]);
    }
  });
});

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

async function run(argv: string[], prompt: Prompt = NOT_A_TERMINAL) {
  let out = "";
  let err = "";
  const exit = await runConsole([...argv, "--server-data", server.serverData], { prompt, output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) } });
  return { exit, out, err };
}

describe("the console", () => {
  test("agent rules ls|add|rm and project trust", async () => {
    const { projectId, dir, session, a } = await setUp();
    expect((await run(["agent", "rules", "add", "deny", "checkpoint", "--project", projectId])).exit).toBe(7);
    const added = await run(["agent", "rules", "add", "deny", "checkpoint", "--project", dir, "--json"], person());
    expect(added.exit).toBe(0);
    const rule = (JSON.parse(added.out) as { rule: Rule }).rule;
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBe("RULE_DENIED");
    const ls = await run(["agent", "rules", "ls", "--project", projectId]);
    expect(ls.out).toContain(`${rule.id}  local  deny checkpoint`);
    expect(ls.out).toContain("project rules (finisher.rules.json): none");
    expect((await run(["agent", "rules", "rm", rule.id], person())).out).toContain(`Removed rule ${rule.id}.`);
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBeUndefined();
    // project trust shows the file's rules, then asks for the project id's first 8 characters.
    writeRules(dir, [{ id: "p1", effect: "deny", command: "checkpoint" }]);
    expect((await run(["project", "trust", projectId])).exit).toBe(7);
    const p = person();
    const trusted = await run(["project", "trust", dir], p);
    expect(trusted.exit).toBe(0);
    expect(trusted.out).toContain(`Trusted project ${projectId}: 1 rules`);
    expect(p.shown.some((t) => t.includes("p1  project  deny checkpoint"))).toBe(true);
    expect(p.asked).toEqual(["Type the project id's first 8 characters to confirm: "]);
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBe("RULE_DENIED");
  });
});

describe("agent migration 0004", () => {
  test("a server before 0004 refuses the database (it would enforce no rules)", () => {
    expect(CONTROL_CHAINS.get("agent")).toMatchObject({ formatVersion: 10, minReaderVersion: 8 });
  });

  test("rootSession and depth: rows before it are their own root at depth 0; new rows too", () => {
    const db = new DatabaseSync(":memory:");
    for (const m of [agent0001, agent0002, agent0003]) m.up(db);
    const insert = (sid: string) =>
      db
        .prepare(
          `INSERT INTO sessions (id, projectId, harness, provider, model, weights, role, mode, baseBranch, baseSeq, status, credentialId, budget, usage, createdBy, createdAt)
           VALUES (?, 'p', 'builtin', 'openrouter', 'm', 'closed', 'editor', 'draft', 'main', 0, 'idle', ?, '{}', '{}', '{}', 0)`,
        )
        .run(sid, sid);
    insert("old");
    agent0004.up(db);
    insert("new");
    const rows = db.prepare("SELECT id, rootSession, depth FROM sessions ORDER BY id").all();
    expect(rows).toEqual([
      { id: "new", rootSession: "new", depth: 0 },
      { id: "old", rootSession: "old", depth: 0 },
    ]);
    for (const t of ["agent_rules", "project_trust"]) expect(db.prepare(`SELECT COUNT(*) AS n FROM ${t}`).get()).toEqual({ n: 0 });
    db.close();
  });
});
