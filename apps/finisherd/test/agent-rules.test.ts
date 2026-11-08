// Command rules (sub-project 7B Task 2, Decisions 3–5 and 7; spec §7.2): globs over command
// names, every layer combined (a `deny` anywhere wins, then `ask`, then `allow`), `where` on
// resolved values, and the stage-5 policy for agent principals whatever the transport.
import { execFile } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { CredentialFile, Principal, Rule, RuleLayer, Session } from "@finisher/contracts";
import { RuleCommand, ruleOf } from "@finisher/contracts";
import { Effect, Schema } from "effect";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { builtinTools } from "../src/agent/builtin/tools.ts";
import { commandsOf } from "../src/agent/command-of.ts";
import { globMatch } from "../src/agent/glob.ts";
import { BUNDLED_ROLES } from "../src/agent/roles.ts";
import { commandRules, NOTHING_RESOLVED, type Resolved, type RuleSource, RuleSet } from "../src/agent/rules.ts";
import { settingsRules } from "../src/agent/trust.ts";
import type { PolicyInput } from "../src/chain/policy.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { childEnv } from "../src/env.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

// --- Units --------------------------------------------------------------------------------

const rule = (layer: RuleLayer, effect: Rule["effect"], command: string, where?: Rule["where"], id = `${layer}.${effect}.${command}`): Rule => ({
  id,
  effect,
  command,
  ...(where === undefined ? {} : { where }),
  layer,
  source: layer,
  createdAt: 0,
});

const SESSION = { id: "01S0000000000000000000000S", projectId: "01P", role: "editor", mode: "draft", provider: "openrouter", rootBranch: "br1" } as unknown as Session;
const AGENT: Principal = { kind: "agent", id: SESSION.id, scopes: ["project.read", "project.write"], projects: ["01P"], branches: ["br1"], human: false };

const decide = (rules: readonly Rule[], command: string, resolved: Resolved = NOTHING_RESOLVED, session: Session = SESSION) => new RuleSet(rules).decide([command], session, resolved);
const effect = (rules: readonly Rule[], command: string) => decide(rules, command).effect;

describe("units", () => {
  test("glob_matches_segments", () => {
    const yes: Array<[string, string]> = [
      ["render", "render"],
      ["asset.*", "asset.add"],
      ["asset.*", "asset.remove"],
      ["**", "asset.add"],
      ["**", "render"],
      ["**", "plugin:@acme/tools/x"],
      ["asset.**", "asset.add"],
      ["*.add", "marker.add"],
      ["caption.*", "caption.set-text"],
      ["rpc:history.*", "rpc:history.undo"],
      // `rpc:` is free text, as `plugin:` and `tool:` are: `rpc:*` is any RPC.
      ["rpc:*", "rpc:history.undo"],
      ["rpc:*", "rpc:jobs.submit"],
      ["plugin:@acme/*", "plugin:@acme/tools/trim-silence"],
      ["plugin:*/publish", "plugin:@acme/tools/publish"],
      ["tool:Bash(curl *)", "tool:Bash(curl https://example.com/a.b)"],
      ["tool:WebFetch", "tool:WebFetch"],
    ];
    const no: Array<[string, string]> = [
      ["render", "render.x"],
      ["render", "rende"],
      ["asset.*", "asset"],
      ["asset.*", "asset.add.x"],
      ["*", "asset.add"],
      ["asset.add", "Asset.add"],
      ["plugin:@acme/*", "plugin:@other/tools/x"],
      ["plugin:*/publish", "plugin:@acme/tools/unpublish"],
      ["tool:Bash(curl *)", "tool:Bash(wget x)"],
      ["tool:Bash(curl *)", "tool:Bash(curl x) && rm"],
      ["asset.(add)", "asset.add"],
      ["rpc:*", "render"],
      ["rpc:jobs.*", "rpc:history.undo"],
    ];
    for (const [p, n] of yes) expect(globMatch(p, n), `${p} ~ ${n}`).toBe(true);
    for (const [p, n] of no) expect(globMatch(p, n), `${p} !~ ${n}`).toBe(false);
    // No pattern makes a long name slow (no backtracking).
    const long = `tool:Bash(${"a".repeat(20_000)})`;
    const t0 = Date.now();
    expect(globMatch(`tool:${"*a".repeat(90)}*b`, long)).toBe(false);
    expect(Date.now() - t0).toBeLessThan(2000);
  });

  test("patterns_that_match_nothing_are_refused: uppercase in a command glob; a tool's name keeps its case", () => {
    const decode = Schema.decodeUnknownExit(RuleCommand);
    for (const bad of ["Render", "asset.Add", "rpc:History.undo", "plugin:@Acme/x", "RPC:history.undo"]) expect(decode(bad)._tag, bad).toBe("Failure");
    for (const good of ["render", "asset.*", "rpc:*", "plugin:@acme/*", "tool:Bash(curl *)", "tool:WebFetch"]) expect(decode(good)._tag, good).toBe("Success");
  });

  test("layers_matrix: deny anywhere, then ask, then allow; no match is allow; ties name the higher layer", () => {
    const layers: RuleLayer[] = ["managed", "local", "project", "user", "role"];
    for (const deny of layers) {
      for (const other of layers) {
        expect(effect([rule(other, "allow", "render"), rule(deny, "deny", "render")], "render"), `${deny} deny vs ${other} allow`).toBe("deny");
        expect(effect([rule(other, "ask", "render"), rule(deny, "deny", "render")], "render"), `${deny} deny vs ${other} ask`).toBe("deny");
        expect(effect([rule(other, "allow", "render"), rule(deny, "ask", "render")], "render"), `${deny} ask vs ${other} allow`).toBe("ask");
      }
    }
    expect(effect([], "render")).toBe("allow");
    expect(effect([rule("user", "deny", "asset.*")], "render")).toBe("allow");
    expect(effect([rule("user", "allow", "render")], "render")).toBe("allow");
    // A role's deny and ask always apply; its allow never overrides a user's ask.
    expect(effect([rule("role", "deny", "plugin.install")], "plugin.install")).toBe("deny");
    expect(effect([rule("role", "ask", "workspace.gc")], "workspace.gc")).toBe("ask");
    expect(effect([rule("role", "allow", "render"), rule("user", "ask", "render")], "render")).toBe("ask");
    // The winning rule is the highest layer's of the winning effect, whatever the order given.
    const o = decide([rule("role", "deny", "render"), rule("user", "deny", "render"), rule("managed", "deny", "**")], "render");
    expect(o).toMatchObject({ effect: "deny", layer: "managed", rule: { id: "managed.deny.**" } });
  });

  test("local_allow_does_not_override_project_deny", () => {
    expect(decide([rule("local", "allow", "render"), rule("project", "deny", "render")], "render")).toMatchObject({ effect: "deny", layer: "project" });
  });

  test("local_allow_does_not_override_project_ask", () => {
    expect(decide([rule("local", "allow", "render"), rule("project", "ask", "render")], "render")).toMatchObject({ effect: "ask", layer: "project" });
  });

  test("user_deny_beats_local_allow", () => {
    expect(decide([rule("local", "allow", "**"), rule("user", "deny", "asset.add")], "asset.add")).toMatchObject({ effect: "deny", layer: "user" });
  });

  test("managed_deny_cannot_be_overridden", () => {
    for (const lifter of ["local", "project", "user", "role"] as const) {
      expect(decide([rule(lifter, "allow", "plugin.install"), rule("managed", "deny", "plugin.*")], "plugin.install"), lifter).toMatchObject({ effect: "deny", layer: "managed" });
      expect(decide([rule(lifter, "allow", "render"), rule("managed", "ask", "render")], "render"), lifter).toMatchObject({ effect: "ask", layer: "managed" });
    }
  });

  test("every name a call could be is evaluated: the strictest wins", () => {
    // A transaction padded with an `asset.add` is `batch`, `asset.add` and `rpc:tx.commit`.
    const names = commandsOf("tx.commit", { tx: { ops: [{ op: "marker.add" }, { op: "asset.add" }] } });
    expect(names).toEqual(expect.arrayContaining(["asset.add", "batch", "rpc:tx.commit"]));
    const set = new RuleSet([rule("user", "deny", "asset.add"), rule("local", "allow", "batch")]);
    expect(set.decide(names, SESSION, NOTHING_RESOLVED)).toMatchObject({ effect: "deny", command: "asset.add" });
    // A rule may name the RPC itself.
    expect(new RuleSet([rule("user", "deny", "rpc:tx.*")]).decide(names, SESSION, NOTHING_RESOLVED).effect).toBe("deny");
  });

  test("render_is_named_from_a_render_job: a deny render refuses it", () => {
    const names = commandsOf("jobs.submit", { kind: "render", project: "01P" });
    expect(names).toEqual(["render", "rpc:jobs.submit"]);
    expect(new RuleSet([rule("local", "deny", "render")]).decide(names, SESSION, NOTHING_RESOLVED)).toMatchObject({ effect: "deny", command: "render" });
    expect(new RuleSet([rule("local", "deny", "render")]).decide(commandsOf("jobs.submit", { kind: "media.look" }), SESSION, NOTHING_RESOLVED).effect).toBe("allow");
  });

  test("bundled_role_defaults_are_the_role_layer", () => {
    const editor = BUNDLED_ROLES.get("editor")!;
    expect(effect(editor.rules, "plugin.install")).toBe("deny");
    expect(effect(editor.rules, "workspace.gc")).toBe("ask");
    // `ask asset.add` of an unlicensed download: inert until 7G resolves the condition.
    expect(effect(editor.rules, "asset.add")).toBe("allow");
    expect(decide(editor.rules, "asset.add", { ...NOTHING_RESOLVED, unlicensedDownload: true }).effect).toBe("ask");
    // `ask render` holds over 10 minutes in draft only.
    const long: Resolved = { ...NOTHING_RESOLVED, minutes: 11 };
    expect(decide(editor.rules, "render", long).effect).toBe("ask");
    expect(decide(editor.rules, "render", { ...NOTHING_RESOLVED, minutes: 10 }).effect).toBe("allow");
    expect(decide(editor.rules, "render", long, { ...SESSION, mode: "auto" }).effect).toBe("allow");
    // Unknown minutes (no render job kind yet): the condition does not hold.
    expect(decide(editor.rules, "render").effect).toBe("allow");
  });
});

const PROJECT_DIR = join(tmpdir(), "finisher-rules-unit-P");

/** Fake projects: `main`, `br1` beneath it, `br2` beneath `br1`; names resolve to ids. */
const PROJECTS: ChainProjects = {
  isOpen: () => true,
  dirOf: (id) => (id === "01P" ? PROJECT_DIR : undefined),
  resolveBranch: (_, ref) => (["main", "br1", "br2"].includes(ref) ? Effect.succeed(ref) : ref === "review" ? Effect.succeed("br2") : Effect.fail({ code: "BRANCH_NOT_FOUND" } as never)),
  branches: () => Effect.succeed([]),
};

const source = (rules: readonly Rule[]): RuleSource => ({ layers: () => ({ rules, projects: new Map() }) });

function policyInput(tag: string, payload: Record<string, unknown>, principal: Principal = AGENT): PolicyInput {
  return { tag: tag as PolicyInput["tag"], rule: ruleOf(tag as PolicyInput["tag"]), principal, payload, projects: PROJECTS, projectIds: ["01P"] };
}

const outcome = (rules: readonly Rule[], tag: string, payload: Record<string, unknown>, options = {}, principal: Principal = AGENT) =>
  Effect.runPromise(commandRules(source(rules), { get: (id) => (id === SESSION.id ? SESSION : undefined) }, options)(policyInput(tag, payload, principal)));

describe("the policy", () => {
  test("where_uses_resolved_values: a branch reference resolved per project, a render's minutes, an asset's bytes", async () => {
    const onReview = [rule("user", "deny", "checkpoint", { branch: "review" })];
    // `review` is br2: a checkpoint on br2 is refused, on br1 it is not.
    expect(await outcome(onReview, "history.checkpoint", { projectId: "01P", branch: "br2", name: "c" })).toMatchObject({ deny: { code: "RULE_DENIED" } });
    expect(await outcome(onReview, "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" })).toBeUndefined();
    // A reference that names no branch holds nowhere.
    expect(await outcome([rule("user", "deny", "checkpoint", { branch: "gone" })], "history.checkpoint", { projectId: "01P", branch: "br2", name: "c" })).toBeUndefined();
    // Reads count their branch too.
    expect(await outcome([rule("user", "deny", "rpc:snapshot.get", { branch: "main" })], "snapshot.get", { projectId: "01P", branch: "main" })).toMatchObject({ deny: { code: "RULE_DENIED" } });
    // minutesOver on the resolved minutes.
    const long = [rule("role", "ask", "render", { minutesOver: 10, mode: "draft" })];
    const minutes = (m: number) => ({ minutes: () => m });
    expect(await outcome(long, "jobs.submit", { kind: "render", project: "01P" }, minutes(11))).toMatchObject({ ask: {} });
    expect(await outcome(long, "jobs.submit", { kind: "render", project: "01P" }, minutes(9))).toBeUndefined();
    expect(await outcome(long, "jobs.submit", { kind: "render", project: "01P" })).toBeUndefined();
    // bytesOver on the files an asset.add adds.
    const big = [rule("user", "deny", "asset.add", { bytesOver: 1000 })];
    const sizes = (n: number) => ({ sizeOf: async () => n });
    const add = { projectId: "01P", tx: { branch: "br1", ops: [addAsset()] } };
    expect(await outcome(big, "tx.commit", add, sizes(5000))).toMatchObject({ deny: { details: { rule: "user.deny.asset.add", layer: "user", command: "asset.add" } } });
    expect(await outcome(big, "tx.commit", add, sizes(10))).toBeUndefined();
    // A relative path is the project folder's, as the store resolves it; an unknown size fails closed.
    const seen: string[] = [];
    const relative = { projectId: "01P", tx: { branch: "br1", ops: [{ ...addAsset(), path: "media/a.mov" }] } };
    expect(await outcome(big, "tx.commit", relative, { sizeOf: async (p: string) => (seen.push(p), 10) })).toBeUndefined();
    expect(seen).toEqual([join(PROJECT_DIR, "media", "a.mov")]);
    expect(await outcome(big, "tx.commit", add, { sizeOf: async () => undefined })).toMatchObject({ deny: { code: "RULE_DENIED" } });
    expect(await outcome([rule("user", "ask", "asset.add", { bytesOver: 1000 })], "tx.commit", add, { sizeOf: async () => undefined })).toMatchObject({ ask: {} });
    // A call that adds no file is 0 bytes: the condition does not hold.
    expect(await outcome([rule("user", "deny", "**", { bytesOver: 0 })], "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" })).toBeUndefined();
    // mode and preset are the session's.
    expect(await outcome([rule("user", "deny", "checkpoint", { mode: "auto" })], "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" })).toBeUndefined();
    expect(await outcome([rule("user", "deny", "checkpoint", { preset: "openrouter" })], "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" })).toMatchObject({ deny: {} });
  });

  test("dry_run_passes_ask: a dry run keeps nothing, so an ask lets it through; a deny still denies it; only an RPC that takes dryRun", async () => {
    const asks = [rule("local", "ask", "checkpoint")];
    const cp = { projectId: "01P", branch: "br1", name: "c" };
    expect(await outcome(asks, "history.checkpoint", cp)).toMatchObject({ ask: {} });
    expect(await outcome(asks, "history.checkpoint", { ...cp, dryRun: true })).toBeUndefined();
    expect(await outcome(asks, "history.checkpoint", { ...cp, dryRun: false })).toMatchObject({ ask: {} });
    expect(await outcome([...asks, rule("user", "deny", "checkpoint")], "history.checkpoint", { ...cp, dryRun: true })).toMatchObject({ deny: { code: "RULE_DENIED" } });
    // `tx.commit` takes no `dryRun` (`fin --dry-run` of an edit never sends one): one sent anyway is a real write and still asks.
    const add = { projectId: "01P", tx: { branch: "br1", ops: [addAsset()] }, dryRun: true };
    expect(await outcome([rule("local", "ask", "asset.add")], "tx.commit", add)).toMatchObject({ ask: {} });
  });

  test("rules apply to agent principals only", async () => {
    const all = [rule("managed", "deny", "**")];
    for (const kind of ["local", "console", "device", "mcp"] as const) {
      expect(await outcome(all, "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" }, {}, { ...AGENT, kind, id: kind }), kind).toBeUndefined();
    }
    // A hand-minted agent credential (no session) still meets the server's layers.
    expect(await outcome(all, "history.checkpoint", { projectId: "01P", branch: "br1", name: "c" }, {}, { ...AGENT, id: "01MINTED00000000000000000A" })).toMatchObject({ deny: {} });
  });
});

// --- On a server --------------------------------------------------------------------------

let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-rules-"));
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
  const projectDir = join(tmp, "P");
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  const opened = await open(server.core, local, projectId);
  const a = await server.agent(opened.credentialPath);
  const owner = await server.console();
  return { local, owner, projectId, projectDir, session: opened.session, credentialPath: opened.credentialPath, a };
}

const checkpoint = (c: NdjsonClient, projectId: string, branch: string) => c.call("history.checkpoint", { projectId, branch, name: `c${++n}`, txId: id(), author: "someone" });
const denied = (e: Exit) => (e.ok ? undefined : { code: e.error.code, rule: (e.error.details as { rule?: string }).rule, layer: (e.error.details as { layer?: string }).layer });

const FIN = fileURLToPath(new URL("../../../target/debug/fin", import.meta.url));
/** CI (`FIN_REQUIRE_SERVER=1`, as `fin`'s own server tests): a missing `fin` fails instead of skipping. */
const REQUIRE_FIN = process.env.FIN_REQUIRE_SERVER === "1";

describe("rules on a server", () => {
  test.skipIf((!existsSync(FIN) && !REQUIRE_FIN) || process.platform === "win32")("rule_holds_over_any_transport: fin, a raw RPC client with the agent credential and the built-in tool", async () => {
    if (!existsSync(FIN)) throw new Error(`FIN_REQUIRE_SERVER=1 but ${FIN} is missing: run \`cargo build -p fin\``);
    const { owner, projectId, projectDir, session, credentialPath, a } = await setUp();
    const root = session.rootBranch!;
    // Allowed before the rule.
    expect(code(await checkpoint(a, projectId, root))).toBeUndefined();
    const { rule: added } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "checkpoint", projectId }));
    expect(added).toMatchObject({ layer: "local", effect: "deny", command: "checkpoint", projectId });
    // 1. A raw RPC client with the agent's credential.
    expect(denied(await checkpoint(a, projectId, root))).toEqual({ code: "RULE_DENIED", rule: added.id, layer: "local" });
    // 2. `fin`, as a harness runs it with the session's environment.
    const workspace = join(projectDir, "workspace", session.id);
    mkdirSync(workspace, { recursive: true });
    const env = { ...childEnv({ FIN_SERVER: server.info.socket, FIN_SESSION: session.id, FIN_PROJECT_DIR: projectDir }), FIN_CREDENTIAL: credentialPath };
    // Asynchronously: the server runs in this process.
    const { exit, stdout } = await new Promise<{ exit: number; stdout: string }>((resolve) =>
      execFile(FIN, ["checkpoint", "by-fin"], { cwd: workspace, env, encoding: "utf8" }, (e, out) => resolve({ exit: e === null ? 0 : Number(e.code), stdout: out })),
    );
    expect(exit).toBe(7);
    expect(stdout).toContain("RULE_DENIED");
    // 3. The built-in agent's `fin` tool.
    const tools = builtinTools({ finPath: FIN, projectDirOf: () => projectDir });
    const call = { sessionId: session.id, projectId, runId: "01RUN0000000000000000000RR", callId: "01CALL00000000000000000001", providerId: "p", cwd: workspace, env };
    const r = await tools.run({ ...call, tool: "fin", input: { argv: ["checkpoint", "by-tool"] } }, new AbortController().signal);
    expect(r.ok).toBe(false);
    expect((r.output as { exit?: number }).exit).toBe(7);
    expect(JSON.stringify(r.output)).toContain("RULE_DENIED");
    // Removed, the command runs again.
    expect(ok<{ removed: boolean }>(await confirmed(owner, "agent.rules.remove", { id: added.id })).removed).toBe(true);
    expect(code(await checkpoint(a, projectId, root))).toBeUndefined();
  });

  test("rules_ignore_people: the local principal's command passes a deny of it; the refusal is audited", async () => {
    const { local, owner, projectId, session, a } = await setUp();
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "**" }));
    expect(code(await checkpoint(local, projectId, session.rootBranch!))).toBeUndefined();
    expect(code(await checkpoint(owner, projectId, "main"))).toBeUndefined();
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBe("RULE_DENIED");
    // Even reads of an agent meet a `**` deny (rules name any RPC).
    expect(code(await a.call("branch.list", { projectId }))).toBe("RULE_DENIED");
    const audit = ok<{ entries: Array<{ kind: string; code?: string; details: { rule?: string } }> }>(await owner.call("auth.audit.list", { kind: "auth.denied" }));
    expect(audit.entries.some((e) => e.code === "RULE_DENIED" && e.details.rule === "commandRules")).toBe(true);
  });

  test("an ask asks a person (a session with no run keeps 5A's ask: Task 4); the editor's inert asset.add ask lets an add through", async () => {
    const { owner, projectId, session, a } = await setUp();
    const add = () => commit(a, projectId, { ...tx(id(), 1, [addAsset(`$b${n}`)]), branch: session.rootBranch! });
    expect(code(await add())).toBeUndefined();
    ok(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "asset.add", sessionId: session.id }));
    const e = await add();
    expect(e.ok ? undefined : [e.error.code, (e.error.details as { reason: string }).reason]).toEqual(["APPROVAL_REQUIRED", expect.stringContaining("(local layer)")]);
  });

  test("role_rules_come_from_the_session_role", async () => {
    const editor = BUNDLED_ROLES.get("editor")!;
    const strict = { ...editor, rules: [...editor.rules, rule("role", "deny", "checkpoint", undefined, "strict.checkpoint")] };
    server.agentOptions = { ...server.agentOptions, roles: new Map([...BUNDLED_ROLES, ["editor", strict]]) };
    const { local, projectId, session, a } = await setUp();
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toEqual({ code: "RULE_DENIED", rule: "strict.checkpoint", layer: "role" });
    // Another role's session does not carry it.
    const other = await open(server.core, local, projectId, { role: "fixer" });
    const b = await server.agent(other.credentialPath);
    expect(code(await checkpoint(b, projectId, other.session.rootBranch!))).toBeUndefined();
  });

  test("user and managed layers come from the server's files; a broken one denies", async () => {
    const { projectId, session, a } = await setUp();
    const { writeFileSync } = await import("node:fs");
    writeFileSync(join(server.serverData, "settings.json"), JSON.stringify({ agents: { rules: [{ id: "u1", effect: "deny", command: "checkpoint" }] } }));
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toEqual({ code: "RULE_DENIED", rule: "u1", layer: "user" });
    writeFileSync(join(server.serverData, "settings.json"), "{}");
    expect(code(await checkpoint(a, projectId, session.rootBranch!))).toBeUndefined();
    writeFileSync(join(server.serverData, "managed-settings.json"), "{ not json");
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toEqual({ code: "RULE_DENIED", rule: "managed.unreadable", layer: "managed" });
    writeFileSync(join(server.serverData, "managed-settings.json"), JSON.stringify({ agents: { rules: [{ id: "m1", effect: "deny", command: "rpc:history.*" }] } }));
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toEqual({ code: "RULE_DENIED", rule: "m1", layer: "managed" });
  });

  test("a changed rules file is picked up: same size, its times put back", async () => {
    const { projectId, session, a } = await setUp();
    const path = join(server.serverData, "settings.json");
    const old = new Date(Date.now() - 3_600_000);
    const write = (id: string) => {
      writeFileSync(path, JSON.stringify({ agents: { rules: [{ id, effect: "deny", command: "checkpoint" }] } }));
      utimesSync(path, old, old);
    };
    write("u1");
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toMatchObject({ rule: "u1" });
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toMatchObject({ rule: "u1" });
    write("u2");
    expect(denied(await checkpoint(a, projectId, session.rootBranch!))).toMatchObject({ rule: "u2" });
  });

  test("an unchanged rules file is parsed once; any change is read again (same size, times put back)", () => {
    const path = join(tmp, "settings.json");
    const old = new Date(Date.now() - 3_600_000);
    const write = (id: string) => {
      writeFileSync(path, JSON.stringify({ agents: { rules: [{ id, effect: "deny", command: "checkpoint" }] } }));
      utimesSync(path, old, old);
    };
    write("u1");
    // Read within RACY_MS of its change: read again each time.
    const fresh = settingsRules(path, "user", "settings.json");
    expect(settingsRules(path, "user", "settings.json")).not.toBe(fresh);
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(Date.now() + 10_000);
      const first = settingsRules(path, "user", "settings.json");
      expect(first.map((r) => r.id)).toEqual(["u1"]);
      expect(settingsRules(path, "user", "settings.json")).toBe(first);
      // The same size and mtime: its ctime (which no one can put back) still moves.
      write("u2");
      expect(settingsRules(path, "user", "settings.json").map((r) => r.id)).toEqual(["u2"]);
    } finally {
      vi.useRealTimers();
    }
  });

  test("a session-less agent credential meets the rules of the project its call names (jobs.submit's and jobs.list's `project`)", async () => {
    const { owner, projectId } = await setUp();
    const minted = ok<{ file: CredentialFile }>(await confirmed(owner, "auth.credential.mint", { kind: "agent", label: "jobs", scopes: ["jobs.submit", "jobs.read"], projects: [projectId] }));
    const { client, exit } = await server.proveAs(minted.file);
    ok(exit);
    const render = () => client.call("jobs.submit", { kind: "render", project: projectId, params: {} });
    expect(code(await render())).not.toBe("RULE_DENIED");
    const { rule: added } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render", projectId }));
    expect(denied(await render())).toEqual({ code: "RULE_DENIED", rule: added.id, layer: "local" });
    expect(code(await client.call("jobs.list", { project: projectId }))).toBeUndefined();
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "rpc:jobs.list", projectId }));
    expect(code(await client.call("jobs.list", { project: projectId }))).toBe("RULE_DENIED");
  });

  test("rule_admin_is_human_only: an agent and fin cannot add or remove rules; the console needs the typed code", async () => {
    const { local, owner, a } = await setUp();
    expect(denied(await a.call("agent.rules.add", { effect: "allow", command: "**" }))).toMatchObject({ code: "SCOPE_DENIED" });
    expect(denied(await a.call("agent.rules.remove", { id: "x" }))).toMatchObject({ code: "SCOPE_DENIED" });
    expect(denied(await local.call("agent.rules.add", { effect: "allow", command: "**" }))).toMatchObject({ code: "SCOPE_DENIED" });
    expect(code(await owner.call("agent.rules.add", { effect: "deny", command: "render" }))).toBe("CONFIRMATION_REQUIRED");
    const { rule: added } = ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render" }));
    const listed = ok<{ rules: Rule[] }>(await local.call("agent.rules.list", {}));
    expect(listed.rules.map((r) => r.id)).toContain(added.id);
    expect(ok<{ removed: boolean }>(await confirmed(owner, "agent.rules.remove", { id: "01nosuchrule000000000000000" })).removed).toBe(false);
  });

  test("agent.rules.list for a session: its role's rules with the others", async () => {
    const { owner, session } = await setUp();
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "render", sessionId: session.id }));
    const { rules } = ok<{ rules: Array<Rule & { applies: boolean; sessionId?: string }> }>(await owner.call("agent.rules.list", { sessionId: session.id }));
    expect(rules.filter((r) => r.layer === "role").map((r) => r.id)).toEqual(expect.arrayContaining(["default.plugin-install", "default.asset-add"]));
    expect(rules.find((r) => r.layer === "local")).toMatchObject({ sessionId: session.id, applies: true });
  });
});
